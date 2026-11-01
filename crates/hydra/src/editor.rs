//! Schematic editor: place, move, rotate and connect components, undo and redo, save revisions
//! to the shared database and restore older ones.
//!
//! Every change goes through a [`Command`] on the in-memory [`Document`]; nothing reaches the
//! database until Save writes a whole new revision.

use eframe::egui::{self, Color32, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};
use hydra_core::{
    Catalogue, CatalogueRef, Circuit, CircuitStatus, Command, Component, ComponentKind, Document,
    ExtraBomLine, Item, Layout, Placement, PortRef, Rotation, SCHEMA_VERSION, Severity, StandIn,
    SymbolDef, ValveStatus, next_id, stand_ins,
};
use hydra_db::{CircuitSummary, OpenedCircuit, RevisionSummary};
use hydra_symbols::{
    Drawing, bounds, drawn_inside_outline, outline_issues, render_circuit, render_parts,
};

use crate::catalogue::Shared;
use crate::db::Request;
use crate::schematic::{draw_fitted, fit, paint_sheet};

/// Parts snap to this grid, in millimetres.
const GRID: f32 = 2.5;
/// How close to a port anchor, in screen points, a press or release must be.
const PORT_REACH: f32 = 8.0;
const ACCENT: Color32 = Color32::from_rgb(0, 120, 215);

/// Outline of a valve by status: red missing, amber part not listed, violet stand-in.
fn status_color(s: ValveStatus) -> Color32 {
    match s {
        ValveStatus::Normal => Color32::TRANSPARENT,
        ValveStatus::Missing => Color32::from_rgb(215, 40, 40),
        ValveStatus::PartNotListed => Color32::from_rgb(230, 150, 0),
        ValveStatus::StandIn => Color32::from_rgb(120, 80, 220),
    }
}

/// A stand-in being swapped in, waiting for the engineer to confirm how its ports connect.
#[derive(Clone)]
struct Swap {
    id: String,
    stand_in: StandIn,
    /// `(old port, new port or none)`, one row per port of the valve as it is now.
    ports: Vec<(String, Option<String>)>,
}

/// A text field whose finished edits become commands.
#[derive(Clone, Debug, PartialEq, Hash)]
enum Field {
    /// Code, description or position of an item.
    Item(String, usize),
    /// Position, quantity, code or description of an extra BOM line.
    Extra(usize, usize),
    Notes,
}

/// A field being typed in: the value it started from and the text so far.
struct Editing {
    field: Field,
    id: egui::Id,
    original: String,
    text: String,
}

/// Where the open circuit lives in the database.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub id: String,
    pub revision: i32,
    pub project: Option<String>,
}

enum Drag {
    /// Moving a part; `grab` is the pointer's offset from the part's origin.
    Move {
        id: String,
        grab: [f32; 2],
        to: Placement,
    },
    /// Drawing a connection from a port.
    Wire {
        from: PortRef,
        start: [f32; 2],
        end: [f32; 2],
    },
    Pan,
}

enum Dialog {
    Open(Option<Vec<CircuitSummary>>),
    History {
        revisions: Option<Vec<RevisionSummary>>,
        preview: Option<(i32, Drawing)>,
    },
    /// Someone saved since this circuit was opened; the user's changes aren't saved.
    Conflict {
        current: i32,
    },
    /// Someone saved since this circuit was opened, so a restore was refused.
    RestoreConflict {
        current: i32,
    },
    /// Unsaved changes would be lost by this action.
    Discard(Pending),
}

#[derive(Clone)]
enum Pending {
    New,
    Open(String),
    /// Close the window.
    Quit,
}

pub struct SchematicEditor {
    doc: Document,
    saved: Option<Saved>,
    selected: Option<String>,
    drag: Option<Drag>,
    pan: Vec2,
    zoom: f32,
    /// View size last fitted to; refit on resize until the user pans or zooms.
    fitted_to: Option<Vec2>,
    /// Canvas rectangle of the last frame, for placing new parts in view.
    view: Rect,
    place: Option<CatalogueRef>,
    /// Text fields, synced from the document while not being edited.
    name: String,
    label: String,
    note: String,
    /// The field drawn with [`Self::field`] that is being typed in.
    editing: Option<Editing>,
    /// Position, quantity, code and description of the extra line about to be added.
    new_line: [String; 4],
    dialog: Option<Dialog>,
    /// The circuit as sent with a save that hasn't been answered yet.
    in_flight: Option<Circuit>,
    /// That save is a copy: the editor moves to it once the save is answered.
    copying: bool,
    /// Id of a circuit being opened or restored. Editing waits, since the reply replaces it.
    awaiting: Option<String>,
    /// The user agreed to close the window despite unsaved changes.
    allow_close: bool,
    status: Option<String>,
    /// Valve statuses for (edit counter, catalogue loads), recomputed only when either moves.
    valve_status: Option<(u64, u64, CircuitStatus)>,
    /// How often `valve_status` was recomputed, for tests.
    status_computed: usize,
    swap: Option<Swap>,
}

impl SchematicEditor {
    pub fn new(circuit: Circuit, saved: Option<Saved>) -> Self {
        Self {
            name: circuit.name.clone(),
            doc: Document::new(circuit),
            saved,
            selected: None,
            drag: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            fitted_to: Some(Vec2::ZERO),
            view: Rect::NOTHING,
            place: None,
            label: String::new(),
            note: String::new(),
            editing: None,
            new_line: Default::default(),
            dialog: None,
            in_flight: None,
            copying: false,
            awaiting: None,
            allow_close: false,
            status: None,
            valve_status: None,
            status_computed: 0,
            swap: None,
        }
    }

    /// Valve statuses of the document against the catalogue, computed once per change.
    fn circuit_status(&mut self, shared: &Shared) -> CircuitStatus {
        let key = (self.doc.generation(), shared.catalogue_loads);
        match &self.valve_status {
            Some((g, l, s)) if (*g, *l) == key => s.clone(),
            _ => {
                let s = CircuitStatus::compute(&self.doc.circuit, &shared.catalogue);
                self.status_computed += 1;
                self.valve_status = Some((key.0, key.1, s.clone()));
                s
            }
        }
    }

    /// Imported by the assistant and not yet saved again by an engineer.
    fn unchecked(&self) -> bool {
        self.doc.circuit.import.is_some() && self.saved.as_ref().is_some_and(|s| s.revision == 1)
    }

    /// Save the imported circuit unchanged as checked, which ends its unchecked state.
    fn mark_checked(&mut self, shared: &Shared) {
        let (Some(db), Some(saved)) = (&shared.db, &self.saved) else {
            return;
        };
        self.in_flight = Some(self.doc.circuit.clone());
        db.send(Request::SaveCircuit {
            id: Some(saved.id.clone()),
            base: saved.revision,
            document: self.doc.circuit.clone(),
            project: saved.project.clone(),
            message: Some("Checked".into()),
        });
    }

    // ---- replies from the database worker ----

    pub fn opened(&mut self, opened: OpenedCircuit) {
        // Only the circuit asked for; a late reply must not replace newer work.
        if self.awaiting.as_deref() != Some(opened.id.as_str()) {
            return;
        }
        let saved = Saved {
            id: opened.id,
            revision: opened.revision,
            project: opened.project,
        };
        let status = format!("Opened revision {}.", saved.revision);
        *self = Self::new(opened.document, Some(saved));
        self.status = Some(status);
    }

    pub fn circuit_saved(&mut self, id: String, revision: i32) {
        // A reply for a document that has since been replaced (New, Open) is not about this one.
        let Some(sent) = self.in_flight.take() else {
            return;
        };
        if std::mem::take(&mut self.copying) {
            // Now editing the copy. Edits made while saving stay, but not the original's undo
            // history, which would bring back its name and import marks.
            let mut current = self.doc.circuit.clone();
            current.name = sent.name.clone();
            current.import = None;
            self.doc = Document::new(current);
            self.valve_status = None;
            self.name = sent.name.clone();
        }
        self.doc.mark_saved_as(sent);
        let project = self.saved.take().and_then(|s| s.project);
        self.saved = Some(Saved {
            id,
            revision,
            project,
        });
        self.note.clear();
        self.status = Some(format!("Saved revision {revision}."));
    }

    pub fn conflict(&mut self, current: i32) {
        self.copying = false;
        if self.in_flight.take().is_some() {
            self.dialog = Some(Dialog::Conflict { current });
        } else if self.awaiting.take().is_some() {
            self.dialog = Some(Dialog::RestoreConflict { current });
        }
    }

    /// A save failed: stop waiting on it and say why.
    pub fn save_failed(&mut self, message: String) {
        self.in_flight = None;
        self.copying = false;
        self.status = Some(message);
    }

    /// Opening, listing or restoring failed: stop waiting on it and say why.
    pub fn circuit_failed(&mut self, message: String) {
        self.awaiting = None;
        if matches!(
            self.dialog,
            Some(
                Dialog::Open(None)
                    | Dialog::History {
                        revisions: None,
                        ..
                    }
            )
        ) {
            self.dialog = None;
        }
        self.status = Some(message);
    }

    /// Whether the window may close now; if not, asks about unsaved changes first.
    pub fn may_close(&mut self) -> bool {
        self.finish_edit();
        if self.allow_close || !self.doc.is_dirty() {
            return true;
        }
        self.dialog = Some(Dialog::Discard(Pending::Quit));
        false
    }

    pub fn circuits(&mut self, list: Vec<CircuitSummary>) {
        if let Some(Dialog::Open(slot)) = &mut self.dialog {
            *slot = Some(list);
        }
    }

    pub fn revisions(&mut self, list: Vec<RevisionSummary>) {
        if let Some(Dialog::History { revisions, .. }) = &mut self.dialog {
            *revisions = Some(list);
        }
    }

    pub fn revision_preview(&mut self, number: i32, circuit: &Circuit, catalogue: &Catalogue) {
        if let Some(Dialog::History { preview, .. }) = &mut self.dialog {
            *preview = Some((number, render_circuit(circuit, catalogue)));
        }
    }

    // ---- editing ----

    pub fn circuit(&self) -> &Circuit {
        &self.doc.circuit
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// An edit made outside the canvas, e.g. on the 3D tab. Refused while a circuit is being
    /// opened, since its reply will replace the document.
    pub fn edit(&mut self, command: Command) -> Result<(), String> {
        if self.awaiting.is_some() {
            return Err("Wait until the circuit has opened.".into());
        }
        self.finish_edit();
        let result = self.doc.apply(command);
        self.prune_selection();
        result
    }

    /// Apply `command` after any edit being typed, so edits land in the order they were made.
    fn apply(&mut self, command: Command) {
        self.finish_edit();
        self.execute(command);
    }

    fn execute(&mut self, command: Command) {
        self.status = self.doc.apply(command).err();
        self.prune_selection();
    }

    /// Undo the last edit; text being typed is finished first, so that is what gets undone.
    fn undo(&mut self) {
        self.finish_edit();
        self.status = self.doc.undo().map(|what| format!("Undid: {what}"));
        self.prune_selection();
    }

    fn redo(&mut self) {
        self.finish_edit();
        self.status = self.doc.redo().map(|what| format!("Redid: {what}"));
        self.prune_selection();
    }

    /// Forget a selected part that no longer exists, e.g. after undoing its placement.
    fn prune_selection(&mut self) {
        let circuit = &self.doc.circuit;
        self.selected = self
            .selected
            .take()
            .filter(|id| circuit.component(id).is_some());
    }

    /// Add a part in the middle of the view and select it. A part landing inside the block's
    /// outline goes in the block, in the same undoable step.
    fn add(&mut self, kind: ComponentKind, catalogue: &Catalogue) {
        let prefix = match &kind {
            ComponentKind::Port { .. } => "X",
            ComponentKind::Valve(v) => v.symbol(catalogue).map_or("V", designator_prefix),
            ComponentKind::Item(item) => designator_prefix(&item.symbol),
        };
        let id = next_id(
            self.doc.circuit.components.iter().map(|c| c.id.as_str()),
            prefix,
        );
        let kind = match kind {
            ComponentKind::Port { .. } => ComponentKind::Port {
                label: id.clone(),
                plugged: false,
            },
            valve => valve,
        };
        let centre = if self.view.is_positive() {
            self.to_sheet(self.view.center())
        } else {
            [0.0, 0.0]
        };
        let placement = Placement {
            x: snap(centre[0]),
            y: snap(centre[1]),
            ..Placement::default()
        };
        let component = Component {
            id: id.clone(),
            kind,
        };
        let in_block = drawn_inside_outline(&self.doc.circuit, &component, placement, catalogue);
        self.apply(Command::Add {
            component,
            placement,
            in_block,
        });
        self.selected = Some(id);
    }

    fn rotate_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let mut placement = self.placement(&id);
        placement.rotation = match placement.rotation {
            Rotation::R0 => Rotation::R90,
            Rotation::R90 => Rotation::R180,
            Rotation::R180 => Rotation::R270,
            Rotation::R270 => Rotation::R0,
        };
        self.apply(Command::Place { id, placement });
    }

    fn mirror_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let mut placement = self.placement(&id);
        placement.mirror = !placement.mirror;
        self.apply(Command::Place { id, placement });
    }

    fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.apply(Command::Remove { id });
        }
    }

    fn placement(&self, id: &str) -> Placement {
        self.doc
            .circuit
            .layout
            .components
            .get(id)
            .copied()
            .unwrap_or_default()
    }

    fn to_sheet(&self, p: Pos2) -> [f32; 2] {
        let v = (p - self.view.min - self.pan) / self.zoom;
        [v.x, v.y]
    }

    fn to_screen(&self, [x, y]: [f32; 2]) -> Pos2 {
        self.view.min + self.pan + Vec2::new(x, y) * self.zoom
    }

    // ---- file actions ----

    fn new_circuit(&mut self) {
        *self = Self::new(empty_circuit(), None);
    }

    /// Run an action that replaces the document, asking first if there are unsaved changes.
    fn guarded(&mut self, action: Pending, shared: &Shared, ctx: &egui::Context) {
        self.finish_edit();
        if self.doc.is_dirty() {
            self.dialog = Some(Dialog::Discard(action));
        } else {
            self.run(action, shared, ctx);
        }
    }

    fn run(&mut self, action: Pending, shared: &Shared, ctx: &egui::Context) {
        self.dialog = None;
        match action {
            Pending::New => self.new_circuit(),
            Pending::Open(id) => {
                if let Some(db) = &shared.db {
                    self.awaiting = Some(id.clone());
                    db.send(Request::OpenCircuit(id));
                }
            }
            Pending::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn can_save(&self, shared: &Shared) -> bool {
        // A clean, saved document has nothing to save; a new one can always be saved.
        let changed = self.saved.is_none() || self.doc.is_dirty() || self.pending();
        shared.db.is_some() && self.in_flight.is_none() && self.awaiting.is_none() && changed
    }

    fn save(&mut self, shared: &Shared, as_copy: bool) {
        self.finish_edit();
        let Some(db) = &shared.db else {
            return;
        };
        // A copy is sent as a new circuit; the open one stays as it is until the copy is saved.
        // Only the original records its import.
        let mut document = self.doc.circuit.clone();
        if as_copy {
            document.name = format!("{} (copy)", document.name);
            document.import = None;
        }
        self.copying = as_copy;
        let target = self.saved.as_ref().filter(|_| !as_copy);
        let note = self.note.trim();
        self.in_flight = Some(document.clone());
        db.send(Request::SaveCircuit {
            id: target.map(|s| s.id.clone()),
            base: target.map_or(0, |s| s.revision),
            document,
            project: self.saved.as_ref().and_then(|s| s.project.clone()),
            message: (!note.is_empty()).then(|| note.to_string()),
        });
    }

    // ---- UI ----

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        // A field that lost focus while not drawn, e.g. behind another tab, finishes its edit.
        self.settle(ui.ctx());
        // Not while a circuit is being opened: its reply would replace the new part.
        if self.awaiting.is_none()
            && let Some(r) = shared.place.take()
        {
            self.add(ComponentKind::valve(r), &shared.catalogue);
        }
        // While a circuit is being opened or restored, its reply will replace the document.
        let locked = self.awaiting.is_some();
        let status = self.circuit_status(shared);
        ui.add_enabled_ui(!locked, |ui| {
            self.toolbar(ui, shared);
            self.import_banner(ui, shared);
            self.selection_bar(ui, &shared.catalogue, &status);
            self.validation(ui, &shared.catalogue, &status);
            self.extra_lines(ui);
            self.notes(ui);
            ui.weak(
                "Drag parts to move them. Drag from a valve port to another port to connect \
             (Shift-drag from an external port). Right-click a port to disconnect. \
             R rotates, Delete removes, double-click fits.",
            );
            self.canvas(ui, &shared.catalogue, &status);
        });
        if !locked {
            self.keys(ui, shared);
        }
        self.dialogs(ui.ctx(), shared);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let online = shared.db.is_some();
        let offline = "Needs the database connection";
        let busy = self.in_flight.is_some() || self.awaiting.is_some();
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").clicked() {
                self.guarded(Pending::New, shared, ui.ctx());
            }
            let open = ui
                .add_enabled(online, egui::Button::new("Open…"))
                .on_disabled_hover_text(offline);
            if open.clicked()
                && let Some(db) = &shared.db
            {
                self.dialog = Some(Dialog::Open(None));
                db.send(Request::ListCircuits);
            }
            let save = ui
                .add_enabled(self.can_save(shared), egui::Button::new("Save"))
                .on_disabled_hover_text(if online { "Nothing to save" } else { offline });
            if save.clicked() {
                self.save(shared, false);
            }
            if ui
                .add_enabled(
                    online && !busy && self.saved.is_some(),
                    egui::Button::new("Save as copy"),
                )
                .clicked()
            {
                self.save(shared, true);
            }
            if busy {
                ui.spinner();
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.note)
                    .hint_text("note for this save")
                    .desired_width(160.0),
            );
            let history = ui
                .add_enabled(
                    online && self.saved.is_some(),
                    egui::Button::new("History…"),
                )
                .on_disabled_hover_text("Save the circuit first");
            if history.clicked()
                && let (Some(db), Some(saved)) = (&shared.db, &self.saved)
            {
                self.dialog = Some(Dialog::History {
                    revisions: None,
                    preview: None,
                });
                db.send(Request::ListRevisions(saved.id.clone()));
            }
            ui.menu_button("Export", |ui| {
                for kind in [Export::Svg, Export::Pdf, Export::Bom] {
                    if ui.button(kind.label()).clicked() {
                        ui.close();
                        self.export(kind, &shared.catalogue);
                    }
                }
            });
            ui.separator();
            let name = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
            if name.lost_focus() && self.name.trim() != self.doc.circuit.name {
                self.apply(Command::Rename {
                    name: self.name.clone(),
                });
            }
            if !name.has_focus() {
                self.name = self.doc.circuit.name.clone();
            }
            let state = match (&self.saved, self.doc.is_dirty()) {
                (Some(s), false) => format!("revision {}", s.revision),
                (Some(s), true) => format!("revision {}, unsaved changes", s.revision),
                (None, _) => "not saved yet".to_string(),
            };
            ui.weak(state);
        });
        ui.horizontal_wrapped(|ui| {
            let undo = ui.add_enabled(self.doc.undo_label().is_some(), egui::Button::new("Undo"));
            let undo = match self.doc.undo_label() {
                Some(what) => undo.on_hover_text(format!("Undo {what}")),
                None => undo,
            };
            if undo.clicked() {
                self.undo();
            }
            if ui
                .add_enabled(self.doc.redo_label().is_some(), egui::Button::new("Redo"))
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            let catalogue = &shared.catalogue;
            let choice =
                |r: &Option<CatalogueRef>| match r.as_ref().and_then(|r| catalogue.valve_type(r)) {
                    Some(v) => format!("{} {} (v{})", v.series, v.code, v.version),
                    None => "Choose a valve type".to_string(),
                };
            egui::ComboBox::from_id_salt("place-valve")
                .selected_text(choice(&self.place))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for v in latest_drawable(catalogue) {
                        let r = Some(v.reference());
                        let text = choice(&r);
                        ui.selectable_value(&mut self.place, r, text);
                    }
                });
            if ui
                .add_enabled(self.place.is_some(), egui::Button::new("Place"))
                .clicked()
                && let Some(r) = self.place.clone()
            {
                self.add(ComponentKind::valve(r), catalogue);
            }
            if ui.button("Add port").clicked() {
                self.add(
                    ComponentKind::Port {
                        label: String::new(),
                        plugged: false,
                    },
                    catalogue,
                );
            }
            ui.menu_button("Add part", |ui| {
                let parts = [
                    ("Test point", SymbolDef::TestPoint),
                    ("Fixed orifice", SymbolDef::Orifice { adjustable: false }),
                    (
                        "Hydraulic motor",
                        SymbolDef::HydraulicMotor {
                            bidirectional: false,
                            variable: false,
                            drain: false,
                        },
                    ),
                    ("Pump", SymbolDef::Pump { variable: false }),
                ];
                for (label, symbol) in parts {
                    if ui.button(label).clicked() {
                        ui.close();
                        let item = Item {
                            symbol,
                            label: None,
                            code: None,
                            description: None,
                            position: None,
                        };
                        self.add(ComponentKind::Item(item), catalogue);
                    }
                }
            });
            if let Some(s) = &self.status {
                ui.weak(s);
            }
        });
    }

    /// Ask where to save, then write the circuit as it is now, unsaved edits included.
    fn export(&mut self, kind: Export, catalogue: &Catalogue) {
        let ext = kind.extension();
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.{ext}", file_stem(&self.doc.circuit.name)))
            .add_filter(ext.to_uppercase(), &[ext])
            .save_file()
        else {
            return;
        };
        let written = export_bytes(kind, &self.doc.circuit, catalogue)
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
        self.status = Some(match written {
            Ok(()) => format!("Exported {}.", path.display()),
            Err(e) => format!("Could not export {}: {e}", path.display()),
        });
    }

    /// For a circuit the assistant imported: say so until an engineer has checked it.
    fn import_banner(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        if !self.unchecked() {
            return;
        }
        let ready = shared.db.is_some()
            && self.in_flight.is_none()
            && self.awaiting.is_none()
            && !self.doc.is_dirty()
            && !self.pending();
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Imported by the AI assistant, unchecked",
            );
            let check = ui
                .add_enabled(ready, egui::Button::new("Mark checked"))
                .on_hover_text("Saves this revision as checked against the drawing")
                .on_disabled_hover_text("Save or undo your changes first");
            if check.clicked() {
                self.mark_checked(shared);
            }
        });
    }

    /// A text field for `field` whose edit becomes one command and one undo step once finished,
    /// not one per keystroke. Escape drops the edit. Its width is the ui's `text_edit_width`.
    fn field(&mut self, ui: &mut egui::Ui, field: Field, label: egui::Id, multiline: bool) {
        let current = self.field_text(&field).unwrap_or_default();
        let mine = self.editing.as_ref().filter(|e| e.field == field);
        let original = mine.map_or(current.clone(), |e| e.original.clone());
        let mut text = mine.map_or(current, |e| e.text.clone());
        let edit = if multiline {
            egui::TextEdit::multiline(&mut text)
        } else {
            egui::TextEdit::singleline(&mut text)
        };
        let response = ui.add(edit.id_salt(&field)).labelled_by(label);
        // Once it loses focus, `settle` finishes the edit at the start of the next frame.
        if !response.has_focus() {
            return;
        }
        // Focus moved here straight from another field: that edit is finished.
        if self.editing.as_ref().is_some_and(|e| e.field != field) {
            self.finish_edit();
        }
        self.editing = Some(Editing {
            field,
            id: response.id,
            original,
            text,
        });
    }

    /// What `field` shows now, or `None` once it no longer exists.
    fn field_text(&self, field: &Field) -> Option<String> {
        let circuit = &self.doc.circuit;
        match field {
            Field::Item(id, f) => match &circuit.component(id)?.kind {
                ComponentKind::Item(item) => Some(item_texts(item)[*f].clone()),
                _ => None,
            },
            Field::Extra(i, f) => Some(extra_texts(circuit.bom_extra.get(*i)?)[*f].clone()),
            Field::Notes => Some(circuit.import.as_ref()?.notes.join("\n")),
        }
    }

    /// A typed change is waiting to be applied.
    fn pending(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| e.text.trim() != e.original.trim())
    }

    /// Finish the edit of a field that no longer has focus, or drop it on Escape.
    fn settle(&mut self, ctx: &egui::Context) {
        let Some(e) = &self.editing else {
            return;
        };
        if ctx.memory(|m| m.has_focus(e.id)) {
            return;
        }
        // Taken, so the same Escape doesn't also deselect the part.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.editing = None;
        } else {
            self.finish_edit();
        }
    }

    /// Apply the field being typed in, unless what it shows changed meanwhile: the text was
    /// typed over the old value and could land on the wrong thing.
    fn finish_edit(&mut self) {
        let Some(e) = self.editing.take() else {
            return;
        };
        if e.text.trim() == e.original.trim() {
            return;
        }
        if self.field_text(&e.field).as_ref() != Some(&e.original) {
            let what = match &e.field {
                Field::Item(id, _) => id.clone(),
                Field::Extra(i, _) => format!("Extra line {}", i + 1),
                Field::Notes => "The import notes".into(),
            };
            self.status = Some(format!(
                "{what} changed while you were editing; type the change again."
            ));
            return;
        }
        let command = match e.field {
            Field::Item(id, f) => {
                let Some(ComponentKind::Item(item)) =
                    self.doc.circuit.component(&id).map(|c| &c.kind)
                else {
                    return;
                };
                let mut texts = item_texts(item);
                texts[f] = e.text;
                let [code, description, position] = texts;
                Command::SetItemDetails {
                    id,
                    code,
                    description,
                    position,
                }
            }
            Field::Extra(index, f) => {
                let mut texts = extra_texts(&self.doc.circuit.bom_extra[index]);
                texts[f] = e.text;
                match extra_line(texts) {
                    Ok(line) => Command::SetExtraLine { index, line },
                    Err(why) => {
                        self.status = Some(why);
                        return;
                    }
                }
            }
            Field::Notes => Command::SetNotes {
                notes: e.text.lines().map(String::from).collect(),
            },
        };
        self.execute(command);
    }

    /// The parts-list lines that aren't drawn, such as the block, plugs and coils.
    fn extra_lines(&mut self, ui: &mut egui::Ui) {
        let count = self.doc.circuit.bom_extra.len();
        egui::CollapsingHeader::new(format!("Extra BOM lines ({count})"))
            .id_salt("bom-extra")
            .show(ui, |ui| {
                let mut remove = None;
                let mut add = false;
                egui::ScrollArea::vertical()
                    .id_salt("bom-extra-scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        egui::Grid::new("bom-extra-grid").show(ui, |ui| {
                            let heads = ["Position", "Quantity", "Code", "Description"]
                                .map(|h| ui.strong(h).id);
                            ui.end_row();
                            let widths = [50.0, 50.0, 140.0, 220.0];
                            for i in 0..count {
                                for f in 0..4 {
                                    ui.spacing_mut().text_edit_width = widths[f];
                                    self.field(ui, Field::Extra(i, f), heads[f], false);
                                }
                                if ui.button("Delete line").clicked() {
                                    remove = Some(i);
                                }
                                ui.end_row();
                            }
                            for (f, text) in self.new_line.iter_mut().enumerate() {
                                ui.add(egui::TextEdit::singleline(text).desired_width(widths[f]))
                                    .labelled_by(heads[f]);
                            }
                            add = ui.button("Add line").clicked();
                            ui.end_row();
                        });
                    });
                if self.new_line.iter().any(|t| !t.trim().is_empty()) {
                    ui.weak("Click Add line to add it.");
                }
                // A cell finished by the same click is applied first, by `apply`.
                if let Some(index) = remove {
                    self.apply(Command::RemoveExtraLine { index });
                }
                if add {
                    match extra_line(self.new_line.clone()) {
                        Ok(line) => {
                            self.apply(Command::AddExtraLine { line });
                            if self.status.is_none() {
                                self.new_line = Default::default();
                            }
                        }
                        Err(why) => self.status = Some(why),
                    }
                }
            });
    }

    /// An imported circuit's notes, one per line.
    fn notes(&mut self, ui: &mut egui::Ui) {
        let Some(import) = &self.doc.circuit.import else {
            return;
        };
        egui::CollapsingHeader::new(format!("Import notes ({})", import.notes.len()))
            .id_salt("import-notes")
            .show(ui, |ui| {
                let label = ui
                    .weak("What the assistant couldn't read or wasn't sure of, one note per line")
                    .id;
                ui.spacing_mut().text_edit_width = ui.available_width();
                self.field(ui, Field::Notes, label, true);
            });
    }

    fn selection_bar(&mut self, ui: &mut egui::Ui, catalogue: &Catalogue, status: &CircuitStatus) {
        let Some(c) = self
            .selected
            .as_ref()
            .and_then(|id| self.doc.circuit.component(id))
            .cloned()
        else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(&c.id);
            match &c.kind {
                ComponentKind::Valve(v) => {
                    match &v.valve_type {
                        Some(t) => ui.monospace(t.to_string()),
                        None => ui.label("not in the catalogue"),
                    };
                    if let Some(p) = &v.part {
                        ui.monospace(p);
                    }
                    if let Some(r) = &v.requested {
                        let asked = ui.label(format!("requested {}", r.code));
                        if !r.description.is_empty() {
                            asked.on_hover_text(&r.description);
                        }
                    }
                    if let Some(s) = status.status(&c.id).filter(|s| *s != ValveStatus::Normal) {
                        ui.colored_label(status_color(s), s.words());
                    }
                }
                ComponentKind::Port { label, .. }
                | ComponentKind::Item(hydra_core::Item {
                    label: Some(label), ..
                }) => {
                    ui.label("Label");
                    let edit =
                        ui.add(egui::TextEdit::singleline(&mut self.label).desired_width(80.0));
                    if edit.lost_focus()
                        && self.label.trim() != label
                        && !self.label.trim().is_empty()
                    {
                        self.apply(Command::SetLabel {
                            id: c.id.clone(),
                            label: self.label.trim().to_string(),
                        });
                    }
                    if !edit.has_focus() {
                        self.label = label.clone();
                    }
                }
                ComponentKind::Item(_) => {}
            }
            if let ComponentKind::Port { plugged, .. } = c.kind {
                let mut p = plugged;
                if ui
                    .checkbox(&mut p, "Plugged")
                    .on_hover_text(
                        "Closed with a plug, e.g. a rail drilled out to the block's edge",
                    )
                    .changed()
                {
                    self.apply(Command::SetPlugged {
                        id: c.id.clone(),
                        plugged: p,
                    });
                }
            }
            if matches!(c.kind, ComponentKind::Item(_)) {
                for (f, (name, width)) in
                    [("Code", 120.0), ("Description", 180.0), ("Position", 40.0)]
                        .into_iter()
                        .enumerate()
                {
                    let label = ui.label(name).id;
                    ui.spacing_mut().text_edit_width = width;
                    self.field(ui, Field::Item(c.id.clone(), f), label, false);
                }
            }
            let mut inside = self.doc.circuit.in_block.contains(&c.id);
            if ui
                .checkbox(&mut inside, "In block")
                .on_hover_text(
                    "Inside the manifold block: the schematic draws the block's outline around it, \
                     and a valve in the block is expected on a face of the 3D block",
                )
                .changed()
            {
                self.apply(Command::SetInBlock {
                    id: c.id.clone(),
                    inside,
                });
            }
            if ui.button("Rotate").clicked() {
                self.rotate_selected();
            }
            if ui.button("Mirror").clicked() {
                self.mirror_selected();
            }
            if ui.button("Delete").clicked() {
                self.delete_selected();
            }
        });
        if status
            .status(&c.id)
            .is_some_and(|s| s != ValveStatus::Normal)
        {
            self.stand_in_row(ui, &c, catalogue, status);
        }
    }

    /// Catalogue valves that could replace the selected one, and the port mapping to confirm.
    fn stand_in_row(
        &mut self,
        ui: &mut egui::Ui,
        c: &Component,
        catalogue: &Catalogue,
        status: &CircuitStatus,
    ) {
        if let Some(mut swap) = self.swap.take().filter(|s| s.id == c.id) {
            let new_ports: Vec<String> = catalogue
                .symbol_of(&swap.stand_in.valve_type)
                .map(|d| d.ports().into_iter().map(String::from).collect())
                .unwrap_or_default();
            let mut done = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Connect {}'s ports as:", c.id));
                for (old, new) in &mut swap.ports {
                    ui.label(format!("{old} →"));
                    egui::ComboBox::from_id_salt(("port-map", old.as_str()))
                        .selected_text(new.as_deref().unwrap_or("—"))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(new, None, "—");
                            for p in &new_ports {
                                ui.selectable_value(new, Some(p.clone()), p);
                            }
                        });
                }
                if ui.button("Swap").clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
            match done {
                Some(true) => self.apply(Command::Use {
                    id: c.id.clone(),
                    valve_type: swap.stand_in.valve_type.clone(),
                    part: swap.stand_in.part.clone(),
                    stand_in: !swap.stand_in.exact,
                    port_map: swap
                        .ports
                        .into_iter()
                        .filter_map(|(o, n)| Some((o, n?)))
                        .collect(),
                }),
                Some(false) => {}
                None => self.swap = Some(swap),
            }
            return;
        }
        let available = status.valves.get(&c.id).is_some_and(|(_, a)| a.is_some());
        match stand_ins(&self.doc.circuit, &c.id, catalogue) {
            Ok(list) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Use instead:");
                    for s in list.into_iter().take(8) {
                        let name = catalogue
                            .valve_type(&s.valve_type)
                            .map_or(s.valve_type.to_string(), |v| {
                                format!("{} {}", v.series, v.code)
                            });
                        let mut text = match &s.part {
                            Some(p) => format!("{name} ({p})"),
                            None => name,
                        };
                        if s.exact && available {
                            text += ", now available";
                        }
                        if s.rewire {
                            text += ", rewire";
                        }
                        if ui.button(text).clicked() {
                            let old = c.ports(catalogue).unwrap_or_default();
                            let ports = old
                                .iter()
                                .map(|o| {
                                    let new = s.port_map.iter().find(|(p, _)| p == o);
                                    (o.to_string(), new.map(|(_, n)| n.clone()))
                                })
                                .collect();
                            self.swap = Some(Swap {
                                id: c.id.clone(),
                                stand_in: s.clone(),
                                ports,
                            });
                        }
                    }
                });
            }
            Err(why) => {
                ui.weak(why.to_string());
            }
        }
    }

    fn validation(&self, ui: &mut egui::Ui, catalogue: &Catalogue, status: &CircuitStatus) {
        let issues: Vec<(Severity, String)> = catalogue
            .problems()
            .into_iter()
            .map(|p| (Severity::Error, format!("catalogue: {p}")))
            .chain(
                self.doc
                    .circuit
                    .validate(catalogue)
                    .iter()
                    .chain(&outline_issues(&self.doc.circuit, catalogue))
                    .chain(&status.issues)
                    .map(|i| (i.severity(), i.to_string())),
            )
            .collect();
        let errors = issues.iter().filter(|(s, _)| *s == Severity::Error).count();
        let title = format!(
            "Validation: {errors} errors, {} warnings",
            issues.len() - errors
        );
        egui::CollapsingHeader::new(title)
            .id_salt("validation")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for (severity, message) in &issues {
                            let color = match severity {
                                Severity::Error => ui.visuals().error_fg_color,
                                Severity::Warning => ui.visuals().warn_fg_color,
                            };
                            ui.colored_label(color, message);
                        }
                    });
            });
    }

    fn canvas(&mut self, ui: &mut egui::Ui, catalogue: &Catalogue, status: &CircuitStatus) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.view = rect;

        // What to draw: the document, with a part being dragged shown at its new spot.
        let mut shown = self.doc.circuit.clone();
        if let Some(Drag::Move { id, to, .. }) = &self.drag {
            shown.layout.components.insert(id.clone(), *to);
        }
        let drawing = render_circuit(&shown, catalogue);
        let parts = render_parts(&shown, catalogue);

        if response.double_clicked() || self.fitted_to.is_some_and(|s| s != rect.size()) {
            (self.pan, self.zoom) = fit(&drawing.primitives, rect.size());
            self.fitted_to = Some(rect.size());
        }
        let reach = PORT_REACH / self.zoom;
        let shift = ui.input(|i| i.modifiers.shift);

        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let p = self.to_sheet(origin);
            self.drag = Some(
                match (port_at(&parts, &shown, p, reach, shift), part_at(&parts, p)) {
                    (Some((from, start)), _) => Drag::Wire {
                        from,
                        start,
                        end: p,
                    },
                    (None, Some(id)) => {
                        let place = self.placement(&id);
                        self.selected = Some(id.clone());
                        Drag::Move {
                            grab: [p[0] - place.x, p[1] - place.y],
                            to: place,
                            id,
                        }
                    }
                    (None, None) => Drag::Pan,
                },
            );
        }
        if let (Some(drag), Some(pointer)) = (&mut self.drag, response.interact_pointer_pos()) {
            let v = (pointer - rect.min - self.pan) / self.zoom;
            let p = [v.x, v.y];
            match drag {
                Drag::Move { grab, to, .. } => {
                    to.x = snap(p[0] - grab[0]);
                    to.y = snap(p[1] - grab[1]);
                }
                Drag::Wire { end, .. } => *end = p,
                Drag::Pan => {
                    self.pan += response.drag_delta();
                    self.fitted_to = None;
                }
            }
        }
        if response.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Move { id, to, .. }) if to != self.placement(&id) => {
                    self.apply(Command::Place { id, placement: to });
                }
                Some(Drag::Wire { from, end, .. }) => {
                    // Any port is a valid target, including external ones.
                    if let Some((to, _)) = port_at(&parts, &shown, end, reach, true)
                        && to != from
                    {
                        self.apply(Command::Connect { a: from, b: to });
                    }
                }
                _ => {}
            }
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.selected = part_at(&parts, self.to_sheet(pos));
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((port, _)) = port_at(&parts, &shown, self.to_sheet(pos), reach, true)
        {
            self.apply(Command::Disconnect { port });
        }
        if let Some(pointer) = response.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let zoom = (self.zoom * (scroll * 0.002).exp() * pinch).clamp(0.05, 200.0);
            let factor = zoom / self.zoom;
            if factor != 1.0 {
                let anchor = pointer - rect.min;
                self.pan = anchor - (anchor - self.pan) * factor;
                self.zoom = zoom;
                self.fitted_to = None;
            }
        }

        let painter = ui.painter_at(rect);
        paint_sheet(&painter, rect, self.pan, self.zoom, &drawing.primitives);
        for (id, part) in &parts {
            if let Some(s) = status.status(id).filter(|s| *s != ValveStatus::Normal)
                && let Some((min, max)) = bounds(&part.primitives)
            {
                let r = Rect::from_min_max(self.to_screen(min), self.to_screen(max)).expand(7.0);
                painter.rect_stroke(
                    r,
                    3.0,
                    Stroke::new(2.0, status_color(s)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        let accent = Stroke::new(1.5, ACCENT);
        if let Some((_, part)) = self
            .selected
            .as_ref()
            .and_then(|id| parts.iter().find(|(p, _)| p == id))
            && let Some((min, max)) = bounds(&part.primitives)
        {
            let r = Rect::from_min_max(self.to_screen(min), self.to_screen(max)).expand(4.0);
            painter.rect_stroke(r, 2.0, accent, egui::StrokeKind::Outside);
        }
        if let Some(Drag::Wire { start, end, .. }) = &self.drag {
            painter.line_segment([self.to_screen(*start), self.to_screen(*end)], accent);
        }
        let hover = response.hover_pos().map(|p| self.to_sheet(p));
        let target =
            hover.and_then(|p| port_at(&parts, &shown, p, reach, shift || self.drag.is_some()));
        if let Some((_, at)) = target {
            painter.circle_stroke(self.to_screen(at), 5.0, accent);
        }
    }

    fn keys(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        // Not while typing, and not behind a dialog that is asking about this document.
        if self.dialog.is_some() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (save, redo, undo, delete, rotate, mirror, escape) = ui.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::CTRL, Key::Y);
            (
                i.consume_key(Modifiers::COMMAND, Key::S),
                redo,
                i.consume_key(Modifiers::COMMAND, Key::Z),
                i.consume_key(Modifiers::NONE, Key::Delete)
                    || i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::R),
                i.consume_key(Modifiers::NONE, Key::M),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if save && self.can_save(shared) {
            self.save(shared, false);
        }
        if redo {
            self.redo();
        } else if undo {
            self.undo();
        }
        if delete {
            self.delete_selected();
        }
        if rotate {
            self.rotate_selected();
        }
        if mirror {
            self.mirror_selected();
        }
        if escape {
            self.drag = None;
            self.selected = None;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, shared: &Shared) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut open = true;
        // Actions that replace the document: `guard` asks about unsaved changes first.
        let mut action: Option<Pending> = None;
        let mut guard: Option<Pending> = None;
        let mut restoring = false;
        let mut close = false;
        match dialog {
            Dialog::Open(list) => {
                egui::Window::new("Open circuit")
                    .open(&mut open)
                    .show(ctx, |ui| match list {
                        None => {
                            ui.spinner();
                        }
                        Some(list) if list.is_empty() => {
                            ui.weak("No circuits saved yet.");
                        }
                        Some(list) => {
                            egui::Grid::new("circuits").striped(true).show(ui, |ui| {
                                for h in ["Name", "Project", "Revision", "Saved", "By", ""] {
                                    ui.strong(h);
                                }
                                ui.end_row();
                                for c in list.iter() {
                                    ui.label(&c.name);
                                    ui.label(c.project.as_deref().unwrap_or(""));
                                    ui.label(c.revision.to_string());
                                    ui.label(&c.saved_at);
                                    ui.label(&c.saved_by);
                                    if ui.button("Open").clicked() {
                                        guard = Some(Pending::Open(c.id.clone()));
                                    }
                                    ui.end_row();
                                }
                            });
                        }
                    });
            }
            Dialog::History { revisions, preview } => {
                let (Some(saved), Some(db)) = (&self.saved, &shared.db) else {
                    self.dialog = None;
                    return;
                };
                let dirty = self.doc.is_dirty();
                egui::Window::new("Revision history")
                    .open(&mut open)
                    .default_width(520.0)
                    .show(ctx, |ui| {
                        let Some(revisions) = revisions else {
                            ui.spinner();
                            return;
                        };
                        if dirty {
                            ui.weak("Save or undo your changes before restoring.");
                        }
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                egui::Grid::new("revisions").striped(true).show(ui, |ui| {
                                    for r in revisions.iter() {
                                        ui.label(format!("#{}", r.number));
                                        ui.label(&r.created_at);
                                        ui.label(&r.created_by);
                                        let note = match r.restored_from {
                                            Some(n) => format!("restored from #{n}"),
                                            None => r.message.clone().unwrap_or_default(),
                                        };
                                        ui.label(note);
                                        if ui.button("Preview").clicked() {
                                            db.send(Request::OpenRevision(
                                                saved.id.clone(),
                                                r.number,
                                            ));
                                        }
                                        let restorable = !dirty && r.number != saved.revision;
                                        if ui
                                            .add_enabled(restorable, egui::Button::new("Restore"))
                                            .clicked()
                                        {
                                            db.send(Request::RestoreRevision {
                                                id: saved.id.clone(),
                                                number: r.number,
                                                base: saved.revision,
                                            });
                                            restoring = true;
                                            close = true;
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        if let Some((number, drawing)) = preview {
                            ui.separator();
                            ui.label(format!("Revision {number}"));
                            let width = ui.available_width();
                            draw_fitted(ui, Vec2::new(width, 260.0), &drawing.primitives);
                        }
                    });
            }
            Dialog::Conflict { current } => {
                let current = *current;
                egui::Window::new("Circuit changed")
                    .collapsible(false)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        ui.label(format!(
                            "Someone saved revision {current} since you opened this circuit. \
                             Your changes are not saved."
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Reload, discarding my changes").clicked()
                                && let Some(saved) = &self.saved
                            {
                                action = Some(Pending::Open(saved.id.clone()));
                            }
                            if ui.button("Save mine as a copy").clicked() {
                                close = true;
                                self.save(shared, true);
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::RestoreConflict { current } => {
                let current = *current;
                egui::Window::new("Circuit changed")
                    .collapsible(false)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        ui.label(format!(
                            "Someone saved revision {current} since you opened this circuit, \
                             so nothing was restored. Reload to see it, then restore again."
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Reload").clicked()
                                && let Some(saved) = &self.saved
                            {
                                action = Some(Pending::Open(saved.id.clone()));
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Discard(pending) => {
                let pending = pending.clone();
                egui::Window::new("Unsaved changes")
                    .collapsible(false)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        ui.label(format!(
                            "Discard your unsaved changes to {}?",
                            self.doc.circuit.name
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Discard").clicked() {
                                action = Some(pending.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
        }
        if restoring {
            self.awaiting = self.saved.as_ref().map(|s| s.id.clone());
        }
        if let Some(action) = guard {
            self.dialog = None;
            self.guarded(action, shared, ctx);
        } else if let Some(action) = action {
            self.run(action, shared, ctx);
        } else if close || !open {
            self.dialog = None;
        }
    }
}

/// An item's code, description and position as text.
fn item_texts(item: &Item) -> [String; 3] {
    [&item.code, &item.description, &item.position].map(|t| t.clone().unwrap_or_default())
}

/// An extra line from its position, quantity, code and description as typed.
fn extra_line(
    [position, quantity, code, description]: [String; 4],
) -> Result<ExtraBomLine, String> {
    let quantity = quantity
        .trim()
        .parse()
        .map_err(|_| "The quantity must be a whole number such as 2.".to_string())?;
    Ok(ExtraBomLine {
        position: Some(position),
        quantity,
        code: Some(code),
        description: Some(description),
    })
}

/// An extra line's position, quantity, code and description as text.
fn extra_texts(line: &ExtraBomLine) -> [String; 4] {
    let text = |t: &Option<String>| t.clone().unwrap_or_default();
    [
        text(&line.position),
        line.quantity.to_string(),
        text(&line.code),
        text(&line.description),
    ]
}

fn snap(v: f32) -> f32 {
    (v / GRID).round() * GRID
}

#[derive(Clone, Copy, Debug)]
enum Export {
    Svg,
    Pdf,
    Bom,
}

impl Export {
    fn label(self) -> &'static str {
        match self {
            Export::Svg => "SVG…",
            Export::Pdf => "PDF…",
            Export::Bom => "Bill of materials (CSV)…",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Export::Svg => "svg",
            Export::Pdf => "pdf",
            Export::Bom => "csv",
        }
    }
}

/// The exported file's contents.
fn export_bytes(kind: Export, circuit: &Circuit, catalogue: &Catalogue) -> Result<Vec<u8>, String> {
    let drawing = || render_circuit(circuit, catalogue).primitives;
    match kind {
        Export::Svg => Ok(hydra_symbols::to_svg(&drawing()).into_bytes()),
        Export::Pdf => hydra_symbols::to_pdf(&drawing()),
        Export::Bom => Ok(hydra_core::bom_csv(&circuit.bom(catalogue)).into_bytes()),
    }
}

/// The circuit name as a file name: without characters Windows or macOS refuse.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect();
    // Windows drops trailing dots and spaces.
    let stem = stem.trim().trim_end_matches(['.', ' ']);
    if stem.is_empty() { "circuit" } else { stem }.to_string()
}

pub fn empty_circuit() -> Circuit {
    Circuit {
        schema: SCHEMA_VERSION,
        assembly: None,
        in_block: Default::default(),
        name: "Untitled circuit".into(),
        components: Vec::new(),
        nets: Vec::new(),
        layout: Layout::default(),
        bom_extra: Vec::new(),
        import: None,
    }
}

/// Designator prefix for a new part drawn with `def`.
fn designator_prefix(def: &SymbolDef) -> &'static str {
    match def {
        SymbolDef::Check { .. } | SymbolDef::PilotCheck { .. } => "K",
        SymbolDef::Relief { .. } | SymbolDef::Counterbalance => "RV",
        SymbolDef::Orifice { .. } => "O",
        SymbolDef::FlowControl { .. } => "FC",
        SymbolDef::TestPoint => "M",
        SymbolDef::HydraulicMotor { .. } => "HM",
        SymbolDef::Pump { .. } => "PU",
        SymbolDef::DirectionalValve(_) | SymbolDef::Box { .. } => "V",
    }
}

/// Valve types worth offering for placement: the latest version of each key that has a symbol.
fn latest_drawable(catalogue: &Catalogue) -> Vec<&hydra_core::ValveType> {
    let mut latest: Vec<&hydra_core::ValveType> = Vec::new();
    for v in catalogue
        .valve_types
        .iter()
        .filter(|v| catalogue.symbol_of(&v.reference()).is_some())
    {
        match latest.iter_mut().find(|l| l.key == v.key) {
            Some(l) if l.version < v.version => *l = v,
            Some(_) => {}
            None => latest.push(v),
        }
    }
    latest.sort_by_key(|v| (&v.series, &v.code));
    latest
}

/// The topmost part whose drawing covers `p`, with a millimetre of slack.
fn part_at(parts: &[(String, Drawing)], p: [f32; 2]) -> Option<String> {
    parts.iter().rev().find_map(|(id, d)| {
        let (min, max) = bounds(&d.primitives)?;
        let inside = (min[0] - 1.0..=max[0] + 1.0).contains(&p[0])
            && (min[1] - 1.0..=max[1] + 1.0).contains(&p[1]);
        inside.then(|| id.clone())
    })
}

/// The port anchor nearest `p` within `reach`. External ports (whose anchor is the whole part)
/// only count when `include_external`, so that dragging one moves it instead.
fn port_at(
    parts: &[(String, Drawing)],
    circuit: &Circuit,
    p: [f32; 2],
    reach: f32,
    include_external: bool,
) -> Option<(PortRef, [f32; 2])> {
    let external = |id: &str| {
        matches!(
            circuit.component(id).map(|c| &c.kind),
            Some(ComponentKind::Port { .. })
        )
    };
    parts
        .iter()
        .filter(|(id, _)| include_external || !external(id))
        .flat_map(|(id, d)| d.ports.iter().map(move |(port, at)| (id, port, *at)))
        .map(|(id, port, at)| ((at[0] - p[0]).hypot(at[1] - p[1]), id, port, at))
        .filter(|(dist, ..)| *dist <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id, port, at)| {
            let port = PortRef {
                component: id.clone(),
                port: port.clone(),
            };
            (port, at)
        })
}

#[cfg(test)]
mod tests {
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    use hydra_core::PORT_PIN;

    use super::*;

    type State = (SchematicEditor, Shared);

    fn shared() -> Shared {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
        Shared::new(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
    }

    fn harness() -> Harness<'static, State> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1100.0, 750.0))
            .build_ui_state(
                |ui, (e, s): &mut State| e.ui(ui, s),
                (SchematicEditor::new(empty_circuit(), None), shared()),
            );
        h.run();
        // A fixed zoom, so ports a few millimetres apart are far enough apart on screen.
        let e = &mut h.state_mut().0;
        (e.zoom, e.pan, e.fitted_to) = (6.0, Vec2::new(500.0, 300.0), None);
        h.run();
        h
    }

    fn anchor(h: &Harness<State>, component: &str, port: &str) -> Pos2 {
        let (e, s) = h.state();
        let parts = render_parts(&e.doc.circuit, &s.catalogue);
        let (_, d) = parts.iter().find(|(id, _)| id == component).unwrap();
        let at = d.ports.iter().find(|(p, _)| p == port).unwrap().1;
        e.to_screen(at)
    }

    fn drag(h: &mut Harness<State>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.drag_at(from);
        h.run();
        for t in [0.25, 0.5, 0.75, 1.0] {
            h.hover_at(from + (to - from) * t);
            h.run();
        }
        h.drop_at(to);
        h.run();
    }

    fn click(h: &mut Harness<State>, at: Pos2, button: egui::PointerButton) {
        h.hover_at(at);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        h.run();
    }

    fn nets(h: &Harness<State>) -> Vec<Vec<String>> {
        h.state()
            .0
            .doc
            .circuit
            .nets
            .iter()
            .map(|n| n.ports.iter().map(|p| p.to_string()).collect())
            .collect()
    }

    #[test]
    fn place_move_connect_undo_rotate_disconnect_delete() {
        let mut h = harness();

        // Place an external port, then drag it down: it moves and snaps to the grid.
        h.get_by_label("Add port").click();
        h.run();
        let x1 = anchor(&h, "X1", PORT_PIN);
        drag(&mut h, x1, x1 + Vec2::new(0.0, 150.0));
        let moved = h.state().0.placement("X1");
        assert!(moved.y > 20.0, "X1 moved down: {moved:?}");
        assert_eq!(moved.y % GRID, 0.0, "snapped to the grid");

        // Place a 4/2 valve from the toolbar.
        h.state_mut().0.place = Some(CatalogueRef::new("vt-demo-4-2", 1));
        h.get_by_label("Place").click();
        h.run();
        assert!(h.state().0.doc.circuit.component("V1").is_some());

        // Drag from the valve's P port onto the external port: they connect.
        let (p, x1) = (anchor(&h, "V1", "P"), anchor(&h, "X1", PORT_PIN));
        // Drop a few points off the port, as a hand would.
        drag(&mut h, p + Vec2::new(2.0, 1.0), x1 + Vec2::new(-3.0, 4.0));
        assert_eq!(nets(&h), [["V1.P", "X1.port"]]);

        // Undo and redo from the keyboard.
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert!(nets(&h).is_empty());
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        h.run();
        assert_eq!(nets(&h).len(), 1);

        // Select the valve by clicking its body and rotate it with R.
        let body = anchor(&h, "V1", "P") + Vec2::new(0.0, -6.0 * 8.0);
        click(&mut h, body, egui::PointerButton::Primary);
        assert_eq!(h.state().0.selected.as_deref(), Some("V1"));
        h.key_press(Key::R);
        h.run();
        assert_eq!(h.state().0.placement("V1").rotation, Rotation::R90);
        assert_eq!(nets(&h).len(), 1, "rotating keeps connections");

        // Right-click the external port to disconnect it.
        let x1 = anchor(&h, "X1", PORT_PIN);
        click(&mut h, x1, egui::PointerButton::Secondary);
        assert!(nets(&h).is_empty());

        // Select it and delete it.
        click(&mut h, x1, egui::PointerButton::Primary);
        assert_eq!(h.state().0.selected.as_deref(), Some("X1"));
        h.key_press(Key::Delete);
        h.run();
        assert!(h.state().0.doc.circuit.component("X1").is_none());
        assert!(h.state().0.doc.is_dirty());
    }

    #[test]
    fn dragging_empty_sheet_pans_and_bad_drops_do_nothing() {
        let mut h = harness();
        h.get_by_label("Add port").click();
        h.run();
        let before = h.state().0.doc.circuit.clone();
        let pan = h.state().0.pan;
        drag(&mut h, Pos2::new(150.0, 600.0), Pos2::new(250.0, 650.0));
        assert_ne!(h.state().0.pan, pan, "empty sheet pans");
        // A wire dropped on nothing connects nothing.
        h.state_mut().0.place = Some(CatalogueRef::new("vt-demo-check", 1));
        h.get_by_label("Place").click();
        h.run();
        let a = anchor(&h, "K1", "A");
        drag(&mut h, a, a + Vec2::new(200.0, 200.0));
        assert!(h.state().0.doc.circuit.nets.is_empty());
        assert_eq!(
            h.state().0.doc.circuit.components.len(),
            before.components.len() + 1
        );
    }

    #[test]
    fn placing_from_the_catalogue_tab_and_unique_designators() {
        let mut h = harness();
        h.state_mut().1.place = Some(CatalogueRef::new("vt-demo-relief", 1));
        h.run();
        h.state_mut().1.place = Some(CatalogueRef::new("vt-demo-relief", 1));
        h.run();
        let ids: Vec<_> = h
            .state()
            .0
            .doc
            .circuit
            .components
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, ["RV1", "RV2"]);
        assert!(h.state().1.place.is_none());
    }

    #[test]
    fn hit_testing_prefers_valve_ports_and_skips_external_ones_unless_asked() {
        let s = shared();
        let mut e = SchematicEditor::new(empty_circuit(), None);
        e.add(
            ComponentKind::Port {
                label: String::new(),
                plugged: false,
            },
            &s.catalogue,
        );
        let parts = render_parts(&e.doc.circuit, &s.catalogue);
        let centre = [0.0, 0.0];
        assert_eq!(part_at(&parts, centre).as_deref(), Some("X1"));
        assert!(port_at(&parts, &e.doc.circuit, centre, 1.0, false).is_none());
        let (port, _) = port_at(&parts, &e.doc.circuit, centre, 1.0, true).unwrap();
        assert_eq!(port.to_string(), "X1.port");
        assert!(part_at(&parts, [50.0, 50.0]).is_none());
    }

    #[test]
    fn saving_offline_is_disabled_and_new_asks_before_discarding() {
        let mut h = harness();
        h.get_by_label("Add port").click();
        h.run();
        assert!(h.state().0.doc.is_dirty());
        // Clicking Save offline does nothing.
        h.get_by_label("Save").click();
        h.run();
        assert!(h.state().0.in_flight.is_none());
        // New with unsaved changes asks first; Cancel keeps the work.
        h.get_by_label("New").click();
        h.run();
        h.get_by_label("Cancel").click();
        h.run();
        assert_eq!(h.state().0.doc.circuit.components.len(), 1);
        h.get_by_label("New").click();
        h.run();
        h.get_by_label("Discard").click();
        h.run();
        assert!(h.state().0.doc.circuit.components.is_empty());
    }

    #[test]
    fn exports_svg_pdf_and_bill_of_materials() {
        let catalogue = shared().catalogue;
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/circuits/demo-manifold.json"
        );
        let circuit = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let export = |kind| export_bytes(kind, &circuit, &catalogue).unwrap();

        let svg = String::from_utf8(export(Export::Svg)).unwrap();
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains(">V1<"), "designators are drawn");
        assert!(export(Export::Pdf).starts_with(b"%PDF-"));
        let csv = String::from_utf8(export(Export::Bom)).unwrap();
        assert!(
            csv.starts_with("\u{feff}Item,Position,Quantity,Designators"),
            "{csv}"
        );
        assert_eq!(csv.lines().count(), circuit.bom(&catalogue).len() + 1);
        assert!(csv.lines().any(|l| l.contains("V1")), "{csv}");
    }

    #[test]
    fn file_names_drop_characters_windows_refuses() {
        assert_eq!(file_stem("Press 3: A/B <v2>?"), "Press 3 AB v2");
        assert_eq!(file_stem("manifold.  "), "manifold");
        assert_eq!(file_stem(" ?* "), "circuit");
    }

    /// An editor on a saved circuit at revision 2, with a worker that goes nowhere.
    /// Its tests step a fixed number of frames: spinners repaint forever, so `run` never settles.
    fn saved_harness() -> Harness<'static, State> {
        let mut h = harness();
        let ctx = egui::Context::default();
        let (e, s) = h.state_mut();
        s.db = Some(crate::db::Db::connect(
            "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
            ctx,
        ));
        e.saved = Some(Saved {
            id: "c1".into(),
            revision: 2,
            project: None,
        });
        h.run_steps(4);
        h
    }

    fn revision(number: i32, restored_from: Option<i32>) -> RevisionSummary {
        RevisionSummary {
            number,
            created_by: "paulo".into(),
            created_at: "2026-10-29 18:00".into(),
            message: Some(format!("save {number}")),
            restored_from,
        }
    }

    #[test]
    fn history_lists_previews_and_restores() {
        let mut h = saved_harness();
        h.get_by_label("History…").click();
        h.run_steps(4);
        assert!(matches!(
            h.state().0.dialog,
            Some(Dialog::History {
                revisions: None,
                ..
            })
        ));
        h.state_mut()
            .0
            .revisions(vec![revision(2, Some(1)), revision(1, None)]);
        h.run_steps(4);
        h.get_by_label("restored from #1");
        h.get_by_label("save 1");

        let (e, s) = h.state_mut();
        let old = e.doc.circuit.clone();
        e.revision_preview(1, &old, &s.catalogue);
        h.run_steps(4);
        h.get_by_label("Revision 1");

        // The current revision can't be restored; an older one can, which closes the dialog.
        let restore = h.get_all_by_label("Restore").collect::<Vec<_>>();
        assert_eq!(restore.len(), 2);
        assert!(
            restore[0].accesskit_node().is_disabled(),
            "revision 2 is current"
        );
        restore[1].click();
        h.run_steps(4);
        assert!(h.state().0.dialog.is_none());
    }

    #[test]
    fn restoring_waits_until_changes_are_saved_or_undone() {
        let mut h = saved_harness();
        h.get_by_label("Add port").click();
        h.run_steps(4);
        h.get_by_label("History…").click();
        h.run_steps(4);
        h.state_mut()
            .0
            .revisions(vec![revision(2, None), revision(1, None)]);
        h.run_steps(4);
        h.get_by_label("Save or undo your changes before restoring.");
        assert!(
            h.get_all_by_label("Restore")
                .all(|b| b.accesskit_node().is_disabled())
        );
    }

    #[test]
    fn conflict_offers_reload_or_save_as_copy() {
        let mut h = saved_harness();
        h.get_by_label("Add port").click();
        h.run_steps(4);
        // Save; the reply is a conflict.
        h.get_by_label("Save").click();
        h.run_steps(4);
        assert!(h.state().0.in_flight.is_some());
        h.state_mut().0.conflict(3);
        h.run_steps(4);
        h.get_by_label_contains("Someone saved revision 3");
        h.get_by_label("Save mine as a copy").click();
        h.run_steps(4);
        assert!(h.state().0.dialog.is_none());
        assert!(h.state().0.in_flight.is_some(), "the copy is being saved");
        // The reply for a copy names a new circuit at revision 1.
        h.state_mut().0.circuit_saved("c2".into(), 1);
        assert_eq!(
            h.state()
                .0
                .saved
                .as_ref()
                .map(|s| (s.id.as_str(), s.revision)),
            Some(("c2", 1))
        );
        assert!(!h.state().0.doc.is_dirty());
    }

    fn opened(id: &str, revision: i32) -> OpenedCircuit {
        OpenedCircuit {
            id: id.into(),
            project: None,
            revision,
            document: empty_circuit(),
        }
    }

    #[test]
    fn open_from_the_dialog_asks_before_discarding_changes() {
        let mut h = saved_harness();
        h.get_by_label("Add port").click();
        h.run_steps(4);
        h.get_by_label("Open…").click();
        h.run_steps(4);
        h.state_mut().0.circuits(vec![CircuitSummary {
            id: "c9".into(),
            name: "Other".into(),
            project: None,
            owner: "paulo".into(),
            revision: 1,
            saved_by: "paulo".into(),
            saved_at: "2026-10-29 18:00".into(),
        }]);
        h.run_steps(4);
        h.get_by_label("Open").click();
        h.run_steps(4);
        assert!(matches!(
            h.state().0.dialog,
            Some(Dialog::Discard(Pending::Open(_)))
        ));
        assert!(h.state().0.awaiting.is_none(), "nothing requested yet");
        h.get_by_label("Discard").click();
        h.run_steps(4);
        assert_eq!(h.state().0.awaiting.as_deref(), Some("c9"));
    }

    #[test]
    fn late_replies_never_land_on_a_different_document() {
        let s = shared();
        let mut e = SchematicEditor::new(
            empty_circuit(),
            Some(Saved {
                id: "A".into(),
                revision: 2,
                project: None,
            }),
        );
        e.add(
            ComponentKind::Port {
                label: String::new(),
                plugged: false,
            },
            &s.catalogue,
        );
        e.in_flight = Some(e.doc.circuit.clone());
        e.new_circuit();
        e.circuit_saved("A".into(), 3);
        assert_eq!(
            e.saved, None,
            "the save of A doesn't make Untitled part of A"
        );
        e.conflict(4);
        assert!(e.dialog.is_none());

        // An Opened reply is applied only for the circuit being waited on.
        e.opened(opened("B", 5));
        assert_eq!(e.saved, None);
        e.awaiting = Some("C".into());
        e.opened(opened("B", 5));
        assert_eq!(e.saved, None);
        e.opened(opened("C", 1));
        assert_eq!(e.saved.as_ref().map(|s| s.id.as_str()), Some("C"));
        assert!(e.awaiting.is_none());
    }

    #[test]
    fn editing_waits_while_a_circuit_is_being_opened() {
        let mut h = saved_harness();
        h.state_mut().0.awaiting = Some("c1".into());
        h.run_steps(4);
        h.get_by_label("Add port").click();
        h.run_steps(4);
        assert!(h.state().0.doc.circuit.components.is_empty());
        // A failed open unlocks the editor again.
        h.state_mut().0.circuit_failed("unknown circuit".into());
        h.run_steps(4);
        h.get_by_label("Add port").click();
        h.run_steps(4);
        assert_eq!(h.state().0.doc.circuit.components.len(), 1);
    }

    #[test]
    fn shortcuts_are_ignored_behind_dialogs_and_selection_follows_undo() {
        let mut h = harness();
        h.get_by_label("Add port").click();
        h.run();
        assert_eq!(h.state().0.selected.as_deref(), Some("X1"));
        h.get_by_label("New").click();
        h.run();
        assert!(matches!(h.state().0.dialog, Some(Dialog::Discard(_))));
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(
            h.state().0.doc.circuit.components.len(),
            1,
            "Delete behind the dialog does nothing"
        );
        h.get_by_label("Cancel").click();
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert!(
            h.state().0.selected.is_none(),
            "the undone part is no longer selected"
        );
    }

    #[test]
    fn save_needs_changes_and_closing_asks_about_them() {
        let mut h = saved_harness();
        h.get_by_label("Save").click();
        h.run_steps(4);
        assert!(
            h.state().0.in_flight.is_none(),
            "a clean saved circuit has nothing to save"
        );
        assert!(h.state_mut().0.may_close());

        h.get_by_label("Add port").click();
        h.run_steps(4);
        assert!(!h.state_mut().0.may_close());
        assert!(matches!(
            h.state().0.dialog,
            Some(Dialog::Discard(Pending::Quit))
        ));
        h.run_steps(4);
        h.get_by_label("Discard").click();
        h.run_steps(4);
        assert!(
            h.state_mut().0.may_close(),
            "the user agreed to lose the changes"
        );
    }

    #[test]
    fn failures_only_cancel_what_they_are_about() {
        let mut h = saved_harness();
        h.get_by_label("Add port").click();
        h.run_steps(4);
        let (e, s) = h.state_mut();
        e.in_flight = Some(e.doc.circuit.clone());
        e.awaiting = Some("B".into());
        // A part requested from the catalogue waits until the open has landed.
        s.place = Some(CatalogueRef::new("vt-demo-check", 1));
        h.run_steps(4);
        assert!(h.state().1.place.is_some());
        h.state_mut().0.save_failed("disk full".into());
        assert_eq!(
            h.state().0.awaiting.as_deref(),
            Some("B"),
            "the open is still on"
        );
        assert!(h.state().0.in_flight.is_none());
        // A failed listing closes its spinner and unlocks the editor.
        h.state_mut().0.dialog = Some(Dialog::Open(None));
        h.state_mut().0.circuit_failed("connection lost".into());
        assert!(h.state().0.dialog.is_none() && h.state().0.awaiting.is_none());
        h.run_steps(4);
        assert!(
            h.state().1.place.is_none(),
            "the waiting part is placed once unlocked"
        );
        assert!(h.state().0.doc.circuit.component("K1").is_some());
    }

    #[test]
    fn a_refused_restore_explains_and_offers_reload() {
        let mut e = SchematicEditor::new(
            empty_circuit(),
            Some(Saved {
                id: "A".into(),
                revision: 2,
                project: None,
            }),
        );
        e.awaiting = Some("A".into());
        e.conflict(3);
        assert!(matches!(
            e.dialog,
            Some(Dialog::RestoreConflict { current: 3 })
        ));
        assert!(e.awaiting.is_none());
    }

    #[test]
    fn save_replies_track_revisions_and_edits_made_while_saving() {
        let s = shared();
        let mut e = SchematicEditor::new(empty_circuit(), None);
        e.add(
            ComponentKind::Port {
                label: String::new(),
                plugged: false,
            },
            &s.catalogue,
        );
        e.in_flight = Some(e.doc.circuit.clone());
        e.add(
            ComponentKind::Port {
                label: String::new(),
                plugged: false,
            },
            &s.catalogue,
        );
        e.circuit_saved("c1".into(), 1);
        assert_eq!(
            e.saved.as_ref().map(|s| (s.id.as_str(), s.revision)),
            Some(("c1", 1))
        );
        assert!(e.doc.is_dirty(), "X2 was added after the save was sent");
        e.undo();
        assert!(!e.doc.is_dirty());

        e.in_flight = Some(e.doc.circuit.clone());
        e.conflict(4);
        assert!(matches!(e.dialog, Some(Dialog::Conflict { current: 4 })));
        assert!(e.in_flight.is_none());
    }

    fn import_harness(revision: Option<i32>, extra_tandem: bool) -> Harness<'static, State> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/circuits/demo-import-manifold.json"
        );
        let circuit = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut shared = shared();
        if extra_tandem {
            let mut other = shared
                .catalogue
                .valve_type(&CatalogueRef::new("vt-demo-4-3-tandem", 1))
                .unwrap()
                .clone();
            other.key = "vt-demo-4-3-tandem-b".into();
            other.code = "4-3-T-B".into();
            shared.catalogue.valve_types.push(other);
        }
        shared.db = Some(crate::db::Db::connect(
            "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
            egui::Context::default(),
        ));
        let saved = revision.map(|revision| Saved {
            id: "c1".into(),
            revision,
            project: None,
        });
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, (e, s): &mut State| {
                    // Not drawn, as behind another dock tab, while `hide` says so.
                    if !ui
                        .ctx()
                        .data(|d| d.get_temp::<bool>(hidden_id()).unwrap_or(false))
                    {
                        e.ui(ui, s);
                    }
                },
                (SchematicEditor::new(circuit, saved), shared),
            );
        h.run_steps(4);
        h
    }

    fn hidden_id() -> egui::Id {
        egui::Id::new("test-hidden")
    }

    /// Stop or start drawing the editor, as switching dock tabs does.
    fn hide(h: &mut Harness<State>, hidden: bool) {
        h.ctx.data_mut(|d| d.insert_temp(hidden_id(), hidden));
        h.run_steps(3);
    }

    fn valve_type_of(h: &Harness<State>, id: &str) -> Option<String> {
        let c = h.state().0.doc.circuit.component(id).unwrap();
        c.valve()
            .and_then(|v| v.valve_type.as_ref())
            .map(ToString::to_string)
    }

    #[test]
    fn an_amber_valve_offers_stand_ins_and_swaps_with_a_port_map() {
        let mut h = import_harness(Some(1), true);
        h.state_mut().0.selected = Some("5.1".into());
        h.run_steps(2);
        assert!(h.query_by_label("PART NOT LISTED").is_some());
        h.get_by_label("DEMO-06 4-3-T-B").click();
        h.run_steps(2);
        assert!(h.query_by_label("Connect 5.1's ports as:").is_some());
        h.get_by_label("Swap").click();
        h.run_steps(2);
        assert_eq!(
            valve_type_of(&h, "5.1").as_deref(),
            Some("vt-demo-4-3-tandem-b@1")
        );
        let v = h
            .state()
            .0
            .doc
            .circuit
            .component("5.1")
            .unwrap()
            .valve()
            .unwrap()
            .clone();
        assert!(v.stand_in);
        assert_eq!(v.requested.unwrap().code, "DEMO-06-4-3-T-24DC");
        // Same ports, so every connection stayed.
        assert_eq!(
            nets(&h)
                .concat()
                .iter()
                .filter(|p| p.starts_with("5.1."))
                .count(),
            4
        );
        h.get_by_label("Undo").click();
        h.run_steps(2);
        assert_eq!(
            valve_type_of(&h, "5.1").as_deref(),
            Some("vt-demo-4-3-tandem@1")
        );
    }

    #[test]
    fn a_missing_valve_with_nothing_to_stand_in_says_what_it_needs() {
        let mut h = import_harness(Some(1), false);
        h.state_mut().0.selected = Some("5.2".into());
        h.run_steps(2);
        assert!(h.query_by_label("MISSING").is_some());
        assert!(
            h.query_by_label("no compatible stand-in: needs directional, ISO 4401-05")
                .is_some()
        );
    }

    #[test]
    fn an_unchecked_import_is_marked_until_checked() {
        let mut h = import_harness(Some(1), false);
        assert!(
            h.query_by_label("Imported by the AI assistant, unchecked")
                .is_some()
        );
        assert!(h.query_by_label("Import notes (2)").is_some());
        // Nothing changed, yet it can be saved as checked.
        h.get_by_label("Mark checked").click();
        h.run_steps(2);
        let sent = h.state().0.in_flight.clone().expect("a save was sent");
        assert_eq!(sent, h.state().0.doc.circuit);
        h.state_mut().0.circuit_saved("c1".into(), 2);
        h.run_steps(2);
        assert!(
            h.query_by_label("Imported by the AI assistant, unchecked")
                .is_none()
        );
        // A later revision of an imported circuit isn't marked.
        let h = import_harness(Some(3), false);
        assert!(h.query_by_label("Mark checked").is_none());
    }

    #[test]
    fn in_block_puts_parts_inside_the_outline_and_flags_parts_drawn_inside_it() {
        let mut h = import_harness(None, false);
        let toggle = |h: &mut Harness<State>, id: &str| {
            h.state_mut().0.selected = Some(id.into());
            h.run_steps(2);
            h.get_by_label("In block").click();
            h.run_steps(2);
        };
        let flagged = |h: &Harness<State>| {
            h.query_by_label_contains("6.1 is outside the block but drawn inside")
                .is_some()
        };
        h.get_by_label_contains("Validation").click();
        h.run_steps(2);
        // P (bottom left), 1.1 (top left) and 4.1 (top right) span the sheet, so 6.1 stands
        // inside the outline, in no empty corner.
        toggle(&mut h, "P");
        toggle(&mut h, "1.1");
        toggle(&mut h, "4.1");
        assert_eq!(
            Vec::from_iter(&h.state().0.doc.circuit.in_block),
            ["1.1", "4.1", "P"]
        );
        assert!(flagged(&h));
        toggle(&mut h, "P");
        assert_eq!(
            Vec::from_iter(&h.state().0.doc.circuit.in_block),
            ["1.1", "4.1"]
        );
        assert!(!flagged(&h));
        h.state_mut().0.undo();
        h.run_steps(2);
        assert!(flagged(&h));
    }

    #[test]
    fn new_parts_landing_inside_the_outline_go_in_the_block() {
        let mut h = harness();
        let in_block = |h: &Harness<State>| -> Vec<String> {
            h.state().0.doc.circuit.in_block.iter().cloned().collect()
        };
        let add_orifice = |h: &mut Harness<State>| {
            h.get_by_label("Add part").click();
            h.run();
            h.get_by_label("Fixed orifice").click();
            h.run();
        };
        // No block parts, so no outline: a new part stays out.
        add_orifice(&mut h);
        assert!(in_block(&h).is_empty());
        let centre = h.state().0.placement("O1");
        h.state_mut().0.delete_selected();
        // Two block ports on either side of the middle of the view draw an outline round it.
        for (id, d) in [("P", -20.0), ("T", 20.0)] {
            h.state_mut().0.apply(Command::Add {
                component: Component {
                    id: id.into(),
                    kind: ComponentKind::Port {
                        label: id.into(),
                        plugged: false,
                    },
                },
                placement: Placement {
                    x: centre.x + d,
                    y: centre.y + d,
                    ..Placement::default()
                },
                in_block: true,
            });
        }
        h.run();
        add_orifice(&mut h);
        assert_eq!(in_block(&h), ["O1", "P", "T"]);
        assert!(
            outline_issues(&h.state().0.doc.circuit, &h.state().1.catalogue).is_empty(),
            "nothing is flagged"
        );
        // One undo takes the part away, and out of the block.
        h.get_by_label("Undo").click();
        h.run();
        assert!(h.state().0.doc.circuit.component("O1").is_none());
        assert_eq!(in_block(&h), ["P", "T"]);
        // With the view moved off to the left of the block, a new part stays out.
        let zoom = h.state().0.zoom;
        h.state_mut().0.pan.x += 60.0 * zoom;
        h.run();
        add_orifice(&mut h);
        assert!(h.state().0.placement("O1").x < centre.x - 40.0);
        assert_eq!(in_block(&h), ["P", "T"]);
    }

    #[test]
    fn mirror_flips_the_selected_part_and_undoes() {
        let mut h = import_harness(None, false);
        let mirrored = |h: &Harness<State>| h.state().0.placement("7.1").mirror;
        h.state_mut().0.selected = Some("7.1".into());
        h.run_steps(2);
        h.key_press(Key::M);
        h.run_steps(2);
        assert!(mirrored(&h));
        h.get_by_label("Mirror").click();
        h.run_steps(2);
        assert!(!mirrored(&h));
        h.state_mut().0.undo();
        h.run_steps(2);
        assert!(mirrored(&h));
    }

    #[test]
    fn a_port_is_plugged_and_unplugged_with_undo() {
        let mut h = import_harness(None, false);
        let plugged = |h: &Harness<State>| {
            matches!(
                h.state().0.doc.circuit.component("P").unwrap().kind,
                ComponentKind::Port { plugged: true, .. }
            )
        };
        h.state_mut().0.selected = Some("P".into());
        h.run_steps(2);
        h.get_by_label("Plugged").click();
        h.run_steps(2);
        assert!(plugged(&h));
        h.state_mut().0.undo();
        h.run_steps(2);
        assert!(!plugged(&h));
    }

    #[test]
    fn warnings_never_block_saving() {
        let mut h = import_harness(Some(1), false);
        let (e, s) = h.state();
        assert!(
            !CircuitStatus::compute(&e.doc.circuit, &s.catalogue)
                .issues
                .is_empty()
        );
        // A real change to a saved circuit full of warnings: Save is offered.
        h.state_mut().0.selected = Some("6.1".into());
        h.run_steps(2);
        h.get_by_label("Rotate").click();
        h.run_steps(2);
        assert!(!h.get_by_label("Save").accesskit_node().is_disabled());
        // Mark checked would also save the edit, so it waits.
        assert!(
            h.get_by_label("Mark checked")
                .accesskit_node()
                .is_disabled()
        );
    }

    #[test]
    fn a_copy_of_an_import_is_unmarked_and_the_original_untouched_until_saved() {
        let mut h = import_harness(Some(1), false);
        let original = h.state().0.doc.circuit.clone();
        h.get_by_label("Save as copy").click();
        h.run_steps(2);
        let sent = h.state().0.in_flight.clone().expect("a save was sent");
        assert_eq!(sent.import, None);
        assert!(sent.name.ends_with("(copy)"));
        assert_eq!(
            h.state().0.doc.circuit,
            original,
            "nothing changes before the reply"
        );
        // A failed save leaves the original as it was.
        h.state_mut().0.save_failed("no".into());
        h.run_steps(2);
        assert_eq!(h.state().0.doc.circuit, original);

        h.get_by_label("Save as copy").click();
        h.run_steps(2);
        h.state_mut().0.circuit_saved("c2".into(), 1);
        h.run_steps(2);
        let e = &h.state().0;
        assert_eq!(e.doc.circuit.import, None);
        assert!(e.doc.circuit.name.ends_with("(copy)"));
        assert!(!e.doc.is_dirty() && e.doc.undo_label().is_none());
        assert!(
            h.query_by_label("Imported by the AI assistant, unchecked")
                .is_none()
        );
    }

    #[test]
    fn statuses_are_computed_once_per_change() {
        let mut h = import_harness(Some(1), false);
        let before = h.state().0.status_computed;
        h.run_steps(5);
        assert_eq!(h.state().0.status_computed, before);
        h.state_mut().0.selected = Some("6.1".into());
        h.run_steps(2);
        assert_eq!(
            h.state().0.status_computed,
            before,
            "selecting isn't a change"
        );
        h.get_by_label("Rotate").click();
        h.run_steps(3);
        assert_eq!(h.state().0.status_computed, before + 1);
    }

    #[test]
    fn generic_parts_are_placed_from_the_add_part_menu() {
        let mut h = harness();
        h.get_by_label("Add part").click();
        h.run();
        h.get_by_label("Test point").click();
        h.run();
        let c = &h.state().0.doc.circuit.components[0];
        assert_eq!(c.id, "M1");
        assert!(matches!(
            &c.kind,
            ComponentKind::Item(Item {
                symbol: SymbolDef::TestPoint,
                ..
            })
        ));
    }

    /// The `n`th text field labelled `label`.
    fn text_field<'h>(h: &'h Harness<State>, label: &'h str, n: usize) -> egui_kittest::Node<'h> {
        use egui::accesskit::Role;
        h.query_all_by_role_and_label(Role::TextInput, label)
            .chain(h.query_all_by_role_and_label(Role::MultilineTextInput, label))
            .nth(n)
            .unwrap()
    }

    /// Replace the text of the `n`th text field labelled `label`, then press `end` if given.
    fn retype(h: &mut Harness<State>, label: &str, n: usize, text: &str, end: Option<Key>) {
        text_field(h, label, n).click();
        h.run_steps(2);
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                h.key_press(Key::Enter);
            }
            // A key at a time, as typed, so a commit per keystroke would show.
            for c in line.chars() {
                h.event(egui::Event::Text(c.into()));
                h.run_steps(1);
            }
        }
        h.run_steps(2);
        if let Some(end) = end {
            h.key_press(end);
            h.run_steps(2);
        }
    }

    #[test]
    fn extra_bom_lines_are_edited_added_deleted_and_undone() {
        let mut h = import_harness(None, false);
        h.get_by_label("Extra BOM lines (2)").click();
        h.run_steps(2);
        let lines = |h: &Harness<State>| h.state().0.doc.circuit.bom_extra.clone();

        // Four plugs become six, in one undo step; the bill of materials follows.
        retype(&mut h, "Quantity", 1, "6", Some(Key::Enter));
        assert_eq!(lines(&h)[1].quantity, 6);
        assert_eq!(h.state().0.doc.undo_label(), Some("Edit extra line 2"));
        let (e, s) = h.state();
        let csv =
            String::from_utf8(export_bytes(Export::Bom, &e.doc.circuit, &s.catalogue).unwrap())
                .unwrap();
        assert!(csv.contains("20,6,,,Plug G1/4,DEMO-PLUG-G1/4"), "{csv}");

        // A bad quantity is refused in words and the field shows the line as it is.
        retype(&mut h, "Quantity", 1, "many", Some(Key::Enter));
        assert_eq!(lines(&h)[1].quantity, 6);
        h.get_by_label("The quantity must be a whole number such as 2.");
        assert_eq!(text_field(&h, "Quantity", 1).value().as_deref(), Some("6"));

        // Add a line from the last row.
        retype(&mut h, "Code", 2, "DEMO-SEAL-12", Some(Key::Tab));
        retype(&mut h, "Quantity", 2, "2", Some(Key::Tab));
        h.get_by_label("Click Add line to add it.");
        h.get_by_label("Add line").click();
        h.run_steps(2);
        assert_eq!(lines(&h).len(), 3);
        assert_eq!(lines(&h)[2].code.as_deref(), Some("DEMO-SEAL-12"));
        assert_eq!(lines(&h)[2].quantity, 2);
        assert!(h.state().0.new_line.iter().all(|t| t.is_empty()));

        // Delete the block line, then undo everything back to the import.
        h.get_all_by_label("Delete line").next().unwrap().click();
        h.run_steps(2);
        assert_eq!(lines(&h)[0].code.as_deref(), Some("DEMO-PLUG-G1/4"));
        for _ in 0..3 {
            h.get_by_label("Undo").click();
            h.run_steps(2);
        }
        assert_eq!(lines(&h)[1].quantity, 4);
        assert_eq!(lines(&h).len(), 2);
        assert!(h.state().0.doc.undo_label().is_none());
    }

    #[test]
    fn an_items_code_is_edited_and_undone() {
        let mut h = import_harness(None, false);
        h.state_mut().0.selected = Some("M1".into());
        h.run_steps(2);
        retype(&mut h, "Code", 0, "DEMO-MP-05", Some(Key::Enter));
        let item = |h: &Harness<State>| match &h.state().0.doc.circuit.component("M1").unwrap().kind
        {
            ComponentKind::Item(i) => i.clone(),
            _ => unreachable!(),
        };
        assert_eq!(item(&h).code.as_deref(), Some("DEMO-MP-05"));
        assert_eq!(
            item(&h).position.as_deref(),
            Some("14"),
            "other fields stay"
        );
        let (e, s) = h.state();
        assert!(
            e.doc
                .circuit
                .bom(&s.catalogue)
                .iter()
                .any(|l| l.part_number.as_deref() == Some("DEMO-MP-05"))
        );
        h.get_by_label("Undo").click();
        h.run_steps(2);
        assert_eq!(item(&h).code.as_deref(), Some("DEMO-MP-04"));
    }

    #[test]
    fn import_notes_are_edited_as_text_in_one_undo_step() {
        let mut h = import_harness(Some(1), false);
        h.get_by_label("Import notes (2)").click();
        h.run_steps(2);
        let label = "What the assistant couldn't read or wasn't sure of, one note per line";
        retype(
            &mut h,
            label,
            0,
            "Row 12 checked (demo).\n\nPlug count from page 2.",
            Some(Key::Tab),
        );
        let notes = |h: &Harness<State>| h.state().0.doc.circuit.import.clone().unwrap().notes;
        assert_eq!(
            notes(&h),
            ["Row 12 checked (demo).", "Plug count from page 2."]
        );
        assert_eq!(h.state().0.doc.undo_label(), Some("Edit the import notes"));
        h.get_by_label("Import notes (2)");
        h.get_by_label("Undo").click();
        h.run_steps(2);
        assert_eq!(notes(&h)[0], "Made-up fixture: no real drawing behind it.");
        assert!(
            h.state().0.doc.undo_label().is_none(),
            "the whole edit was one step"
        );
    }

    const NOTES: &str = "What the assistant couldn't read or wasn't sure of, one note per line";

    fn open_section(h: &mut Harness<State>, title: &str) {
        h.get_by_label(title).click();
        h.run_steps(2);
    }

    fn notes_of(h: &Harness<State>) -> Vec<String> {
        h.state().0.doc.circuit.import.clone().unwrap().notes
    }

    #[test]
    fn escape_drops_an_edit_and_added_blank_space_is_no_change() {
        let mut h = import_harness(None, false);
        h.state_mut().0.selected = Some("M1".into());
        h.run_steps(2);
        retype(&mut h, "Code", 0, "DEMO-MP-99", Some(Key::Escape));
        assert_eq!(
            h.state().0.selected.as_deref(),
            Some("M1"),
            "still selected"
        );
        assert_eq!(
            text_field(&h, "Code", 0).value().as_deref(),
            Some("DEMO-MP-04")
        );
        retype(&mut h, "Code", 0, "DEMO-MP-04 ", Some(Key::Enter));
        let e = &h.state().0;
        assert!(
            e.doc.undo_label().is_none() && e.status.is_none(),
            "{:?}",
            e.status
        );
    }

    #[test]
    fn undo_while_typing_undoes_the_typing_not_another_line() {
        let mut h = import_harness(None, false);
        open_section(&mut h, "Extra BOM lines (2)");
        h.get_all_by_label("Delete line").next().unwrap().click();
        h.run_steps(2);
        // The plugs are the first line now: type over their code, then Undo before finishing.
        retype(&mut h, "Code", 0, "DEMO-PLUG-M8", None);
        h.get_by_label("Undo").click();
        h.run_steps(2);
        let lines = &h.state().0.doc.circuit.bom_extra;
        assert_eq!(lines.len(), 1, "the block stays deleted");
        assert_eq!(lines[0].code.as_deref(), Some("DEMO-PLUG-G1/4"));
        assert_eq!(h.state().0.doc.redo_label(), Some("Edit extra line 1"));
    }

    #[test]
    fn an_edit_over_a_value_that_changed_meanwhile_is_dropped_with_a_reason() {
        let mut h = import_harness(Some(1), false);
        open_section(&mut h, "Import notes (2)");
        retype(&mut h, NOTES, 0, "Mine (demo).", None);
        // A change that didn't come from this field lands before the edit is finished.
        let theirs = "Theirs (demo).".to_string();
        let doc = &mut h.state_mut().0.doc;
        doc.circuit
            .import
            .as_mut()
            .unwrap()
            .notes
            .push(theirs.clone());
        hide(&mut h, true);
        hide(&mut h, false);
        assert_eq!(notes_of(&h).last(), Some(&theirs));
        assert_eq!(
            h.state().0.status.as_deref(),
            Some("The import notes changed while you were editing; type the change again.")
        );
    }

    #[test]
    fn an_edit_left_behind_another_tab_or_selection_is_kept() {
        let mut h = import_harness(Some(1), false);
        open_section(&mut h, "Import notes (2)");
        retype(&mut h, NOTES, 0, "Checked (demo).", None);
        hide(&mut h, true);
        hide(&mut h, false);
        assert_eq!(notes_of(&h), ["Checked (demo)."]);

        h.state_mut().0.selected = Some("M1".into());
        h.run_steps(2);
        retype(&mut h, "Code", 0, "DEMO-MP-05", None);
        h.state_mut().0.selected = Some("X2".into());
        h.run_steps(3);
        let m1 = h
            .state()
            .0
            .doc
            .circuit
            .component("M1")
            .unwrap()
            .kind
            .clone();
        let ComponentKind::Item(m1) = m1 else {
            unreachable!()
        };
        assert_eq!(m1.code.as_deref(), Some("DEMO-MP-05"));
    }

    #[test]
    fn save_takes_the_edit_being_typed() {
        let mut h = import_harness(Some(1), false);
        open_section(&mut h, "Import notes (2)");
        retype(&mut h, NOTES, 0, "Checked (demo).", None);
        assert!(
            h.get_by_label("Mark checked")
                .accesskit_node()
                .is_disabled(),
            "Mark checked saves the circuit unchanged"
        );
        h.get_by_label("Save").click();
        h.run_steps(2);
        let sent = h.state().0.in_flight.clone().expect("a save was sent");
        assert_eq!(sent.import.unwrap().notes, ["Checked (demo)."]);
    }

    #[test]
    fn a_cell_finished_by_clicking_delete_line_is_kept() {
        let mut h = import_harness(None, false);
        open_section(&mut h, "Extra BOM lines (2)");
        retype(&mut h, "Code", 1, "DEMO-PLUG-M8", None);
        h.get_all_by_label("Delete line").next().unwrap().click();
        h.run_steps(2);
        let lines = &h.state().0.doc.circuit.bom_extra;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].code.as_deref(), Some("DEMO-PLUG-M8"));
        assert_eq!(h.state().0.doc.undo_label(), Some("Delete extra line 1"));
    }

    #[test]
    fn clicking_from_one_cell_into_another_keeps_the_first_edit() {
        let mut h = import_harness(None, false);
        open_section(&mut h, "Extra BOM lines (2)");
        // The cell clicked into is drawn before the one typed in.
        retype(&mut h, "Code", 1, "DEMO-PLUG-M8", None);
        retype(&mut h, "Code", 0, "DEMO-BLOCK-460", Some(Key::Enter));
        let codes: Vec<_> = h
            .state()
            .0
            .doc
            .circuit
            .bom_extra
            .iter()
            .map(|l| l.code.clone())
            .collect();
        assert_eq!(
            codes,
            [Some("DEMO-BLOCK-460".into()), Some("DEMO-PLUG-M8".into())]
        );
    }

    #[test]
    fn new_and_closing_ask_about_an_edit_being_typed() {
        let typing = || {
            let mut h = import_harness(Some(1), false);
            open_section(&mut h, "Import notes (2)");
            retype(&mut h, NOTES, 0, "Checked (demo).", None);
            h
        };
        let mut h = typing();
        h.get_by_label("New").click();
        h.run_steps(2);
        assert!(matches!(
            h.state().0.dialog,
            Some(Dialog::Discard(Pending::New))
        ));
        assert_eq!(notes_of(&h), ["Checked (demo)."], "nothing discarded yet");

        let mut h = typing();
        assert!(!h.state_mut().0.may_close());
        assert!(matches!(
            h.state().0.dialog,
            Some(Dialog::Discard(Pending::Quit))
        ));
    }
}
