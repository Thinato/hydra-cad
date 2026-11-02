//! Catalogue tab: search and browse valve types, edit them as new versions, attach datasheets.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align, Color32, FontFamily, Frame, Layout, Margin, RichText, Sense, Vec2,
};
use hydra_core::{Catalogue, CatalogueRef, SymbolDef, ValveType};

use crate::db::{Db, Reply, Request};
use crate::schematic::symbol_preview;
use crate::theme::{self, LIGHT, Mark};

/// What the window says while a file is dragged over it.
const DROP_HINT: &str =
    "Drop a PDF to attach it to the selected valve type,\nor a catalogue JSON file to import it";

/// Catalogue data shared by every tab, and where it comes from.
pub struct Shared {
    pub catalogue: Catalogue,
    /// sha256 of each valve type's datasheet.
    pub datasheets: HashMap<CatalogueRef, String>,
    /// sha256 of the 3D model (glb) of each valve type and block that has one.
    pub valve_models: HashMap<CatalogueRef, String>,
    pub block_models: HashMap<CatalogueRef, String>,
    /// `None` when running offline from a fixture file: browsing works, editing does not.
    pub db: Option<Db>,
    /// Replies still due from workers of earlier sign-ins, until each worker exits.
    pub retired: Vec<Receiver<Reply>>,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// A valve type the user asked to place in the schematic.
    pub place: Option<CatalogueRef>,
    /// Counts catalogue replacements, so anything derived from it can be cached per value.
    pub catalogue_loads: u64,
}

impl Shared {
    /// Offline, until a database connection is set.
    pub fn new(catalogue: Catalogue) -> Self {
        Shared {
            catalogue,
            datasheets: HashMap::new(),
            valve_models: HashMap::new(),
            block_models: HashMap::new(),
            db: None,
            retired: Vec::new(),
            error: None,
            notice: None,
            place: None,
            catalogue_loads: 0,
        }
    }
}

#[derive(Default)]
pub struct CataloguePanel {
    search: String,
    all_versions: bool,
    selected: Option<CatalogueRef>,
    editor: Option<Editor>,
}

struct Editor {
    draft: ValveType,
    is_new: bool,
    mounting: String,
    params: String,
    saving: bool,
    error: Option<String>,
}

impl CataloguePanel {
    /// A save went through: show the new version and close the editor that asked for it.
    pub fn saved(&mut self, r: CatalogueRef) {
        self.selected = Some(r);
        if self.editor.as_ref().is_some_and(|e| e.saving) {
            self.editor = None;
        }
    }

    /// Show a failed save in the editor waiting on it. Returns false if none is.
    pub fn save_failed(&mut self, message: &str) -> bool {
        match &mut self.editor {
            Some(e) if e.saving => {
                e.saving = false;
                e.error = Some(message.to_string());
                true
            }
            _ => false,
        }
    }

    /// The toolbar over the valve-type list, and the selected valve type's detail on the right.
    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let mut place = None;
        {
            let shared: &Shared = shared;
            let catalogue = &shared.catalogue;
            let latest = latest_versions(catalogue);
            let rows = visible_rows(catalogue, &latest, &self.search, self.all_versions);
            egui::Panel::top("catalogue-toolbar")
                .frame(
                    Frame::new()
                        .fill(LIGHT.panel)
                        .inner_margin(Margin::symmetric(12, 8)),
                )
                .show(ui, |ui| {
                    self.toolbar(ui, catalogue, shared.db.is_some(), rows.len())
                });
            egui::Panel::right("catalogue-detail")
                .resizable(true)
                .default_size(400.0)
                .min_size(320.0)
                .max_size(560.0)
                .frame(Frame::new().fill(LIGHT.panel))
                .show(ui, |ui| place = self.detail(ui, shared, &latest));
            ui.painter()
                .rect_filled(ui.available_rect_before_wrap(), 0.0, LIGHT.surface);
            self.list(ui, &rows, &latest, catalogue, &shared.datasheets);
            self.editor_window(ui.ctx(), shared);
        }
        if let Some(r) = place {
            shared.notice = Some(format!(
                "{r} will be placed when you open the Schematic tab."
            ));
            shared.place = Some(r);
        }
    }

    /// An editor is waiting on a save; opening another would lose its reply.
    fn saving(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.saving)
    }

    /// Search, All versions, how many types are shown, and New valve type.
    fn toolbar(&mut self, ui: &mut egui::Ui, catalogue: &Catalogue, online: bool, shown: usize) {
        ui.spacing_mut().interact_size.y = 32.0;
        ui.horizontal(|ui| {
            let label = ui.label(RichText::new("Search").text_style(theme::label()));
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Manufacturer, series, code, mounting")
                    .desired_width(280.0)
                    .min_size(Vec2::new(0.0, 32.0))
                    .vertical_align(Align::Center),
            )
            .labelled_by(label.id);
            ui.checkbox(&mut self.all_versions, "All versions");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let new = ui
                    .add_enabled(
                        online && !self.saving(),
                        egui::Button::new("New valve type"),
                    )
                    .on_hover_text("Start a valve type that isn't in the catalogue yet.")
                    .on_disabled_hover_text(if online { WAITING } else { OFFLINE });
                if new.clicked() {
                    self.editor = Some(Editor::new(new_valve_type(catalogue), true));
                }
                let s = if shown == 1 { "" } else { "s" };
                ui.label(RichText::new(format!("{shown} valve type{s}")).color(LIGHT.secondary));
            });
        });
    }

    /// The valve types as a table: a header row, then one selectable row per valve type.
    fn list(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[&ValveType],
        latest: &HashMap<&str, u32>,
        catalogue: &Catalogue,
        datasheets: &HashMap<CatalogueRef, String>,
    ) {
        let widths = column_widths(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        theme::table_row(ui, LIGHT.panel, |ui| {
            // In line with the rows' cells, which sit inside the rows' 1 px edge.
            ui.add_space(1.0);
            for ((title, _), width) in COLUMNS.iter().zip(widths) {
                theme::cell(ui, width, |ui| {
                    theme::column_title(ui, title);
                });
            }
        });
        if rows.is_empty() {
            Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                let text = if self.search.trim().is_empty() {
                    "The catalogue has no valve types."
                } else {
                    "No valve types match the search."
                };
                ui.label(RichText::new(text).color(LIGHT.secondary));
            });
            return;
        }
        // Every row is one line high, so only the rows in view are laid out.
        let line = ui.text_style_height(&egui::TextStyle::Body);
        let height = line + ROW_PADDING;
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("catalogue-list")
            .auto_shrink(false)
            .show_rows(ui, height, rows.len(), |ui, range| {
                for v in &rows[range] {
                    let r = v.reference();
                    let selected = self.selected.as_ref() == Some(&r);
                    let newer = latest[v.key.as_str()];
                    let row = theme::list_row(ui, selected, &row_name(v), |ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        // Not `horizontal`, whose 28 px minimum would make rows taller than
                        // `height`.
                        let one_line = Vec2::new(ui.available_width(), line);
                        ui.allocate_ui_with_layout(
                            one_line,
                            Layout::left_to_right(Align::Min),
                            |ui| {
                                ui.set_min_height(line);
                                list_cells(
                                    ui,
                                    v,
                                    newer,
                                    catalogue,
                                    datasheets.contains_key(&r),
                                    widths,
                                );
                            },
                        );
                    });
                    if row.clicked() {
                        clicked = Some(r);
                    }
                }
            });
        if clicked.is_some() {
            self.selected = clicked;
        }
    }

    /// The selected valve type. Returns the latest version of it to place in the schematic,
    /// when asked.
    fn detail(
        &mut self,
        ui: &mut egui::Ui,
        shared: &Shared,
        latest: &HashMap<&str, u32>,
    ) -> Option<CatalogueRef> {
        let Some(v) = self
            .selected
            .as_ref()
            .and_then(|r| shared.catalogue.valve_type(r))
            .cloned()
        else {
            theme::section(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Select a valve type in the list to see its symbol, parts and datasheet \
                         here.",
                    )
                    .color(LIGHT.secondary),
                );
            });
            return None;
        };
        egui::ScrollArea::vertical()
            .id_salt("catalogue-detail")
            .auto_shrink(false)
            .show(ui, |ui| self.fields(ui, &v, shared, latest[v.key.as_str()]))
            .inner
    }

    fn fields(
        &mut self,
        ui: &mut egui::Ui,
        v: &ValveType,
        shared: &Shared,
        latest: u32,
    ) -> Option<CatalogueRef> {
        let catalogue = &shared.catalogue;
        let mut place = None;
        // New parts and new versions use the latest version, whichever version is on screen.
        let newest = CatalogueRef::new(&v.key, latest);
        let online = shared.db.is_some();
        theme::section(ui, |ui| {
            ui.heading(format!("{} {}", v.series, v.code));
            ui.label(
                RichText::new(manufacturer_name(catalogue, &v.manufacturer)).color(LIGHT.secondary),
            );
            ui.monospace(v.reference().to_string());
            if latest > v.version {
                theme::marked(
                    ui,
                    Mark::Warning,
                    format!("A newer version exists: {}@{latest}", v.key),
                );
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let refusal = place_refusal(catalogue, &newest);
                let place_button = ui
                    .add_enabled(refusal.is_none(), theme::primary("Place in schematic"))
                    .on_hover_text(format!(
                        "Adds {newest} to the circuit on the Schematic tab."
                    ))
                    .on_disabled_hover_text(refusal.unwrap_or_default());
                if place_button.clicked() {
                    place = Some(newest.clone());
                }
                let label = if latest > v.version {
                    "Edit latest as new version"
                } else {
                    "Edit as new version"
                };
                let edit = ui
                    .add_enabled(
                        online && !self.saving(),
                        egui::Button::new(label).min_size(Vec2::new(0.0, 32.0)),
                    )
                    .on_hover_text(format!(
                        "Saves as {}@{}; {newest} stays as it is.",
                        v.key,
                        latest + 1
                    ))
                    .on_disabled_hover_text(if online { WAITING } else { OFFLINE });
                if edit.clicked()
                    && let Some(base) = catalogue.valve_type(&newest)
                {
                    self.editor = Some(Editor::new(base.clone(), false));
                }
            });
        });

        let def = v.symbol.as_ref().and_then(|s| catalogue.symbol_def(s));
        theme::section(ui, |ui| {
            theme::column_title(ui, "SYMBOL");
            let size = Vec2::new(ui.available_width(), 200.0);
            symbol_preview(ui, size, def, "How Hydra draws it", |ui| {
                symbol_reference(ui, v.symbol.as_ref());
            });
            match (&v.symbol, def) {
                (Some(_), Some(def)) => {
                    ui.label(RichText::new(kind_name(def)).color(LIGHT.secondary));
                }
                (Some(s), None) => {
                    theme::marked(
                        ui,
                        Mark::Error,
                        format!(
                            "Symbol {s} is missing from the catalogue, so this type can't be drawn."
                        ),
                    );
                }
                (None, _) => {
                    theme::marked(
                        ui,
                        Mark::Warning,
                        "Draft: no symbol yet. Give it one in a new version to place it.",
                    );
                }
            }
        });

        theme::section(ui, |ui| {
            theme::column_title(ui, "MOUNTING");
            match &v.mounting {
                Some(m) => ui.monospace(m),
                None => ui.label(RichText::new("Not given").color(LIGHT.secondary)),
            };
        });

        if !v.params.is_null() {
            theme::section(ui, |ui| {
                theme::column_title(ui, "PARAMETERS");
                let params = serde_json::to_string(&v.params).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(params).monospace()).wrap());
            });
        }

        let r = v.reference();
        let parts: Vec<_> = catalogue.parts_of(&r).collect();
        theme::section(ui, |ui| {
            theme::column_title(ui, &format!("PARTS ({})", parts.len()));
            if parts.is_empty() {
                ui.label(RichText::new("No parts listed for this version.").color(LIGHT.secondary));
                return;
            }
            let width = ui.available_width();
            // Part number and options; the row's 12 px margins and gap come off the width.
            let number = ((width - 36.0) * 0.5).max(0.0);
            theme::table(ui, width, |ui| {
                theme::table_row(ui, LIGHT.panel, |ui| {
                    theme::cell(ui, number, |ui| {
                        theme::column_title(ui, "PART NUMBER");
                    });
                    theme::column_title(ui, "OPTIONS");
                });
                for p in parts {
                    theme::table_row(ui, Color32::TRANSPARENT, |ui| {
                        theme::cell(ui, number, |ui| {
                            let text = RichText::new(&p.part_number)
                                .family(FontFamily::Name(theme::MONO_SEMIBOLD.into()));
                            ui.add(egui::Label::new(text).wrap());
                        });
                        let options =
                            RichText::new(options_text(&p.options)).color(LIGHT.secondary);
                        ui.add(egui::Label::new(options).wrap());
                    });
                }
            });
        });

        theme::section(ui, |ui| {
            theme::column_title(ui, "DATASHEET");
            match (shared.datasheets.get(&v.reference()), &shared.db) {
                (Some(sha256), Some(db)) => {
                    let open = egui::Button::new("Open datasheet").min_size(Vec2::new(0.0, 32.0));
                    if ui.add(open).clicked() {
                        db.send(Request::OpenAsset {
                            sha256: sha256.clone(),
                            extension: "pdf",
                        });
                    }
                }
                (None, Some(_)) => {
                    ui.label(
                        RichText::new("No datasheet. Drop a PDF on this window to attach it.")
                            .color(LIGHT.secondary),
                    );
                }
                (_, None) => {
                    ui.label(
                        RichText::new("Datasheets need the database connection.")
                            .color(LIGHT.secondary),
                    );
                }
            }
        });
        place
    }

    fn editor_window(&mut self, ctx: &egui::Context, shared: &Shared) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.is_new {
            "New valve type".to_string()
        } else {
            format!("New version of {}", editor.draft.key)
        };
        let mut open = true;
        let mut cancel = false;
        // Closing is ignored while saving: the reply must find the editor that sent it.
        let saving = editor.saving;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| cancel = editor.ui(ui, shared));
        if (!open || cancel) && !saving {
            self.editor = None;
        }
    }

    /// Files dropped anywhere on the window, whichever tab is showing.
    pub fn handle_dropped_files(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        let (hovering, dropped) =
            ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        if hovering {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop"),
            ));
            // A scrim of ink over the whole window, and the words in the title style on it.
            painter.rect_filled(rect, 0.0, LIGHT.ink.gamma_multiply(0.8));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                DROP_HINT,
                egui::TextStyle::Heading.resolve(&ctx.global_style()),
                LIGHT.surface,
            );
        }
        for file in dropped {
            let Some(db) = &shared.db else {
                shared.error = Some("Importing and attaching need the database connection.".into());
                continue;
            };
            match dropped_file_request(file.path(), file.bytes(), self.selected.as_ref()) {
                Ok(request) => {
                    shared.error = None;
                    db.send(request);
                }
                Err(e) => shared.error = Some(e),
            }
        }
    }
}

/// What a file dropped on the catalogue asks for: a PDF attaches to the selected valve type,
/// a JSON catalogue is imported.
fn dropped_file_request(
    path: &std::path::Path,
    bytes: Result<Vec<u8>, String>,
    selected: Option<&CatalogueRef>,
) -> Result<Request, String> {
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let name = path.display();
    let bytes = bytes.map_err(|e| format!("Could not read {name}: {e}"))?;
    match extension.as_deref() {
        Some("pdf") => {
            let r = selected.ok_or("Select a valve type first, then drop its datasheet.")?;
            Ok(Request::AttachDatasheet(r.clone(), bytes))
        }
        Some("json") => {
            let c: Catalogue = serde_json::from_slice(&bytes)
                .map_err(|e| format!("{name} is not a catalogue file: {e}"))?;
            // Every field has a default, so any JSON object parses; an empty result means the
            // file was something else, e.g. a circuit.
            if c.manufacturers.is_empty() && c.symbol_defs.is_empty() && c.valve_types.is_empty() {
                return Err(format!(
                    "{name} is not a catalogue file: it has no manufacturers, symbols or valve types."
                ));
            }
            Ok(Request::Import(c))
        }
        _ => Err(format!(
            "{name}: drop a PDF datasheet or a catalogue JSON file."
        )),
    }
}

impl Editor {
    fn new(draft: ValveType, is_new: bool) -> Self {
        Editor {
            mounting: draft.mounting.clone().unwrap_or_default(),
            params: if draft.params.is_null() {
                String::new()
            } else {
                serde_json::to_string_pretty(&draft.params).unwrap_or_default()
            },
            draft,
            is_new,
            saving: false,
            error: None,
        }
    }

    /// The fields as a table, the symbol as it will be drawn, what is wrong, then Cancel and
    /// Save as new version. Returns true when the engineer cancels.
    fn ui(&mut self, ui: &mut egui::Ui, shared: &Shared) -> bool {
        let catalogue = &shared.catalogue;
        // Form fields are 32 px.
        ui.spacing_mut().interact_size.y = 32.0;
        let width = ui.available_width().max(360.0);
        // Field and value; the table's edges, the row's 12 px margins and the gap come off.
        let cols = [110.0, (width - 2.0 - 24.0 - 12.0 - 110.0).max(0.0)];
        theme::table(ui, width, |ui| {
            theme::table_row(ui, LIGHT.panel, |ui| {
                theme::cell(ui, cols[0], |ui| {
                    theme::column_title(ui, "FIELD");
                });
                theme::column_title(ui, "VALUE");
            });
            editor_row(ui, cols, "Key", |ui, label| {
                if self.is_new {
                    ui.add(mono_edit(&mut self.draft.key).hint_text("e.g. vt-demo-4-3"))
                        .labelled_by(label);
                } else {
                    ui.horizontal(|ui| {
                        ui.set_min_height(32.0);
                        ui.monospace(&self.draft.key);
                    });
                }
            });
            editor_row(ui, cols, "Manufacturer", |ui, _| {
                egui::ComboBox::from_id_salt("manufacturer")
                    .width(cols[1])
                    .selected_text(manufacturer_name(catalogue, &self.draft.manufacturer))
                    .show_ui(ui, |ui| {
                        for m in &catalogue.manufacturers {
                            ui.selectable_value(
                                &mut self.draft.manufacturer,
                                m.key.clone(),
                                &m.name,
                            );
                        }
                    });
            });
            editor_row(ui, cols, "Series", |ui, label| {
                ui.add(mono_edit(&mut self.draft.series)).labelled_by(label);
            });
            editor_row(ui, cols, "Code", |ui, label| {
                ui.add(mono_edit(&mut self.draft.code)).labelled_by(label);
            });
            editor_row(ui, cols, "Mounting", |ui, label| {
                ui.add(mono_edit(&mut self.mounting).hint_text("e.g. ISO 4401-03"))
                    .labelled_by(label);
            });
            editor_row(ui, cols, "Symbol", |ui, _| {
                let name = |s: &Option<CatalogueRef>| match s {
                    Some(r) => r.to_string(),
                    None => "None (draft)".to_string(),
                };
                egui::ComboBox::from_id_salt("symbol")
                    .width(cols[1])
                    .selected_text(RichText::new(name(&self.draft.symbol)).monospace())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.draft.symbol, None, name(&None));
                        for s in &catalogue.symbol_defs {
                            let text = format!("{} ({})", s.reference(), kind_name(&s.definition));
                            ui.selectable_value(&mut self.draft.symbol, Some(s.reference()), text);
                        }
                    });
            });
            editor_row(ui, cols, "Parameters", |ui, label| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.params)
                        .code_editor()
                        .hint_text("JSON, optional")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label);
            });
        });
        if let Some(def) = self
            .draft
            .symbol
            .as_ref()
            .and_then(|s| catalogue.symbol_def(s))
        {
            ui.add_space(12.0);
            let size = Vec2::new(width, 140.0);
            symbol_preview(ui, size, Some(def), "How Hydra will draw it", |ui| {
                symbol_reference(ui, self.draft.symbol.as_ref());
            });
        }
        if let Some(e) = &self.error {
            ui.add_space(8.0);
            theme::marked(ui, Mark::Error, e.as_str());
        }
        ui.add_space(12.0);
        let mut cancel = false;
        // Dialog buttons are 32 px, the primary last.
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 32.0;
            let back = ui
                .add_enabled(
                    !self.saving,
                    egui::Button::new("Cancel").min_size(Vec2::new(0.0, 32.0)),
                )
                .on_disabled_hover_text(WAITING);
            cancel = back.clicked();
            let save = ui
                .add_enabled(!self.saving, theme::primary("Save as new version"))
                .on_hover_text(
                    "Adds a new version to the catalogue; the versions already there stay as \
                     they are.",
                )
                .on_disabled_hover_text(WAITING);
            if self.saving {
                ui.spinner();
            }
            if save.clicked() {
                match (self.finished(), &shared.db) {
                    (Ok(v), Some(db)) => {
                        self.saving = true;
                        self.error = None;
                        db.send(Request::SaveValveType(v));
                    }
                    (Ok(_), None) => {
                        self.error = Some(
                            "Saving needs the database connection, which is gone. Your edits \
                             stay here; save again once it is back."
                                .into(),
                        )
                    }
                    (Err(e), _) => self.error = Some(e),
                }
            }
        });
        cancel
    }

    /// The draft with its text fields parsed, or what is wrong with it.
    fn finished(&self) -> Result<ValveType, String> {
        let mut v = self.draft.clone();
        v.key = v.key.trim().to_string();
        if v.key.is_empty() || v.key.contains('@') {
            return Err("The key must not be empty or contain '@'.".into());
        }
        v.series = v.series.trim().to_string();
        v.code = v.code.trim().to_string();
        if v.series.is_empty() || v.code.is_empty() {
            return Err("Series and code are required.".into());
        }
        let mounting = self.mounting.trim();
        v.mounting = (!mounting.is_empty()).then(|| mounting.to_string());
        v.params = if self.params.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&self.params)
                .map_err(|e| format!("Parameters are not valid JSON: {e}"))?
        };
        Ok(v)
    }
}

/// One row of the editor's table: the field's label, then the value `value` adds, which it can
/// label with the label's id.
fn editor_row(
    ui: &mut egui::Ui,
    cols: [f32; 2],
    label: &str,
    value: impl FnOnce(&mut egui::Ui, egui::Id),
) {
    theme::table_row(ui, Color32::TRANSPARENT, |ui| {
        let mut id = egui::Id::NULL;
        theme::cell(ui, cols[0], |ui| {
            // Level with the first line of a 32 px field.
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                id = ui.label(RichText::new(label).text_style(theme::label())).id;
            });
        });
        theme::cell(ui, cols[1], |ui| value(ui, id));
    });
}

/// A single-line field in Data type, as wide as its cell.
fn mono_edit(value: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(value)
        .font(egui::TextStyle::Monospace)
        .desired_width(f32::INFINITY)
        .min_size(Vec2::new(0.0, 32.0))
        .vertical_align(Align::Center)
}

const OFFLINE: &str = "Needs the database connection.";
const WAITING: &str = "Waiting for the database.";

/// The list's columns, with their widths when there is room. Manufacturer takes the slack.
const COLUMNS: [(&str, Option<f32>); 7] = [
    ("SERIES", Some(100.0)),
    ("CODE", Some(90.0)),
    ("MANUFACTURER", None),
    ("MOUNTING", Some(100.0)),
    ("VERSION", Some(64.0)),
    ("STATUS", Some(140.0)),
    ("DATASHEET", Some(76.0)),
];
const MANUFACTURER_MIN: f32 = 120.0;
/// A list row's height beyond its line of text: `list_row`'s padding and edge.
const ROW_PADDING: f32 = 18.0;

/// Each column's width in a list `width` wide. When the list is narrower than the columns
/// want, every column shrinks alike and long text is cut short.
fn column_widths(width: f32) -> [f32; 7] {
    // A row's 12 px margins and 1 px edges, and the 12 px gaps between its cells.
    let room = (width - 26.0 - 12.0 * (COLUMNS.len() - 1) as f32).max(0.0);
    let fixed: f32 = COLUMNS.iter().filter_map(|c| c.1).sum();
    let scale = (room / (fixed + MANUFACTURER_MIN)).min(1.0);
    COLUMNS.map(|(_, w)| match w {
        Some(w) => w * scale,
        None => room - fixed * scale,
    })
}

/// One valve type's cells in the list. Text too long for its column is cut short, so every row
/// stays one line high.
fn list_cells(
    ui: &mut egui::Ui,
    v: &ValveType,
    latest: u32,
    catalogue: &Catalogue,
    datasheet: bool,
    widths: [f32; 7],
) {
    let mono =
        |text: &str| RichText::new(text).family(FontFamily::Name(theme::MONO_SEMIBOLD.into()));
    let cells: [RichText; 5] = [
        mono(&v.series),
        mono(&v.code),
        RichText::new(manufacturer_name(catalogue, &v.manufacturer)),
        RichText::new(v.mounting.as_deref().unwrap_or("")).monospace(),
        RichText::new(v.version.to_string()).monospace(),
    ];
    for (text, width) in cells.into_iter().zip(widths) {
        theme::cell(ui, width, |ui| {
            ui.add(egui::Label::new(text).truncate());
        });
    }
    theme::cell(ui, widths[5], |ui| {
        if let Some((mark, words)) = row_status(v, latest, catalogue) {
            // `theme::marked`, cut short instead of wrapped.
            let color = mark.tone().text;
            let text = RichText::new(words)
                .small()
                .family(FontFamily::Name(theme::SEMIBOLD.into()))
                .color(color);
            let line = ui.text_style_height(&egui::TextStyle::Body);
            let row = Vec2::new(ui.available_width(), line);
            ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
                let side = ui.text_style_height(&egui::TextStyle::Small);
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                mark.paint(ui.painter(), rect.shrink(side * 0.2), color);
                ui.add(egui::Label::new(text).truncate());
            });
        }
    });
    theme::cell(ui, widths[6], |ui| {
        if datasheet {
            ui.label(RichText::new("PDF").color(LIGHT.secondary));
        }
    });
}

/// A list row's name for screen readers: series, code and version, so versions differ.
fn row_name(v: &ValveType) -> String {
    format!("{} {}, version {}", v.series, v.code, v.version)
}

/// What the list says about a valve type: its symbol is missing, a newer version exists, or it
/// is a draft without a symbol. A normal valve type says nothing, as a normal valve shows no
/// colour.
fn row_status(v: &ValveType, latest: u32, catalogue: &Catalogue) -> Option<(Mark, String)> {
    match &v.symbol {
        Some(s) if catalogue.symbol_def(s).is_none() => {
            Some((Mark::Error, "Symbol missing".into()))
        }
        _ if latest > v.version => Some((Mark::Warning, format!("Version {latest} is newer"))),
        None => Some((Mark::Warning, "Draft, no symbol".into())),
        Some(_) => None,
    }
}

/// Why the valve type `newest` can't be placed in the schematic, or `None` when it can.
fn place_refusal(catalogue: &Catalogue, newest: &CatalogueRef) -> Option<String> {
    let symbol = catalogue.valve_type(newest).and_then(|t| t.symbol.as_ref());
    match symbol {
        None => Some(format!(
            "{newest} has no symbol, so it can't be drawn. Give it one in a new version."
        )),
        Some(s) if catalogue.symbol_def(s).is_none() => Some(format!(
            "Symbol {s} of {newest} is missing from the catalogue, so it can't be drawn."
        )),
        Some(_) => None,
    }
}

/// A part's options as an engineer reads them: `voltage: 24 V DC · connector: EN 175301-803`.
/// Anything but an object stays JSON; no options is empty.
fn options_text(options: &serde_json::Value) -> String {
    match options {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}: {s}"),
                v => format!("{k}: {v}"),
            })
            .collect::<Vec<_>>()
            .join(" · "),
        other => other.to_string(),
    }
}

/// A symbol preview's reference: the symbol's `key@version`, or that it has none.
fn symbol_reference(ui: &mut egui::Ui, symbol: Option<&CatalogueRef>) {
    match symbol {
        Some(s) => {
            ui.label(
                RichText::new(s.to_string())
                    .monospace()
                    .color(LIGHT.secondary),
            );
        }
        None => {
            theme::marked(ui, Mark::Warning, RichText::new("No symbol").small());
        }
    }
}

/// The latest version of each valve type's key.
fn latest_versions(catalogue: &Catalogue) -> HashMap<&str, u32> {
    let mut latest: HashMap<&str, u32> = HashMap::new();
    for v in &catalogue.valve_types {
        let e = latest.entry(&v.key).or_default();
        *e = (*e).max(v.version);
    }
    latest
}

/// Valve types matching the search, latest version of each key unless `all_versions`.
fn visible_rows<'a>(
    catalogue: &'a Catalogue,
    latest: &HashMap<&str, u32>,
    search: &str,
    all_versions: bool,
) -> Vec<&'a ValveType> {
    let needle = search.trim().to_lowercase();
    let mut rows: Vec<&ValveType> = catalogue
        .valve_types
        .iter()
        .filter(|v| all_versions || latest[v.key.as_str()] == v.version)
        .filter(|v| {
            let haystack = [
                manufacturer_name(catalogue, &v.manufacturer),
                &v.series,
                &v.code,
                &v.key,
                v.mounting.as_deref().unwrap_or(""),
            ];
            needle.is_empty() || haystack.iter().any(|h| h.to_lowercase().contains(&needle))
        })
        .collect();
    rows.sort_by_key(|v| {
        (
            manufacturer_name(catalogue, &v.manufacturer),
            &v.series,
            &v.code,
            v.version,
        )
    });
    rows
}

fn manufacturer_name<'a>(catalogue: &'a Catalogue, key: &'a str) -> &'a str {
    catalogue
        .manufacturers
        .iter()
        .find(|m| m.key == key)
        .map_or(key, |m| &m.name)
}

fn kind_name(def: &SymbolDef) -> &'static str {
    match def {
        SymbolDef::DirectionalValve(_) => "directional valve",
        SymbolDef::Check { .. } => "check",
        SymbolDef::Relief { .. } => "relief",
        SymbolDef::Orifice { .. } => "orifice",
        SymbolDef::FlowControl { .. } => "flow control",
        SymbolDef::PilotCheck { .. } => "pilot-operated check",
        SymbolDef::Counterbalance => "counterbalance",
        SymbolDef::TestPoint => "test point",
        SymbolDef::HydraulicMotor { .. } => "hydraulic motor",
        SymbolDef::Pump { .. } => "pump",
        SymbolDef::Box { .. } => "box",
    }
}

/// An empty draft, pre-filled with the first manufacturer.
fn new_valve_type(catalogue: &Catalogue) -> ValveType {
    ValveType {
        key: String::new(),
        // Base version 0: the database refuses the save if the key already exists.
        version: 0,
        manufacturer: catalogue
            .manufacturers
            .first()
            .map(|m| m.key.clone())
            .unwrap_or_default(),
        series: String::new(),
        code: String::new(),
        symbol: None,
        mounting: None,
        params: serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::accesskit::{Role, Toggled};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    fn catalogue() -> Catalogue {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
        let mut c: Catalogue =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut v2 = c.valve_types[0].clone();
        v2.version = 2;
        v2.code = "Z11-new".into();
        c.valve_types.push(v2);
        c
    }

    #[test]
    fn search_shows_latest_versions_unless_asked() {
        let c = catalogue();
        let latest = latest_versions(&c);
        let visible_rows = |c, search, all| visible_rows(c, &latest, search, all);
        let z11: Vec<_> = visible_rows(&c, "rpe3", false)
            .iter()
            .map(|v| v.reference())
            .collect();
        assert_eq!(z11, [CatalogueRef::new("vt-rpe3-06-z11", 2)]);
        assert_eq!(visible_rows(&c, "RPE3", true).len(), 2);
        // Manufacturer names are searchable, not just keys.
        assert_eq!(visible_rows(&c, "argo-hytos", false).len(), 1);
        assert_eq!(visible_rows(&c, "Argo", false).len(), 1);
        assert_eq!(visible_rows(&c, "", false).len(), c.valve_types.len() - 1);
        assert!(visible_rows(&c, "no such valve", true).is_empty());
    }

    #[test]
    fn dropped_files_attach_or_import() {
        use std::path::Path;
        let z11 = CatalogueRef::new("vt-rpe3-06-z11", 2);
        let pdf = || Ok(b"%PDF-1.4".to_vec());
        assert!(matches!(
            dropped_file_request(Path::new("a/B.PDF"), pdf(), Some(&z11)),
            Ok(Request::AttachDatasheet(r, _)) if r == z11
        ));
        assert_eq!(
            dropped_file_request(Path::new("b.pdf"), pdf(), None)
                .err()
                .unwrap(),
            "Select a valve type first, then drop its datasheet."
        );
        let json = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/catalogue.json"
        ));
        assert!(matches!(
            dropped_file_request(Path::new("cat.json"), Ok(json.unwrap()), None),
            Ok(Request::Import(c)) if c.valve_types.len() == 12
        ));
        let circuit = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/circuits/demo-manifold.json"
        ));
        for bad in [b"[]".to_vec(), b"{not json".to_vec(), circuit.unwrap()] {
            let e = dropped_file_request(Path::new("x.json"), Ok(bad), None)
                .err()
                .unwrap();
            assert!(e.starts_with("x.json is not a catalogue file"), "{e}");
        }
        assert!(dropped_file_request(Path::new("x.step"), pdf(), Some(&z11)).is_err());
        let unreadable = dropped_file_request(Path::new("y.pdf"), Err("denied".into()), Some(&z11));
        assert_eq!(unreadable.err().unwrap(), "Could not read y.pdf: denied");
    }

    #[test]
    fn editor_parses_and_checks_fields() {
        let c = catalogue();
        let mut e = Editor::new(c.valve_types[0].clone(), false);
        e.mounting = "  ".into();
        e.params = r#"{"max_pressure_bar": 350}"#.into();
        let v = e.finished().unwrap();
        assert_eq!(v.mounting, None);
        assert_eq!(v.params["max_pressure_bar"], 350);

        e.params = "{not json".into();
        assert!(
            e.finished()
                .unwrap_err()
                .starts_with("Parameters are not valid JSON")
        );
        e.params.clear();
        e.draft.key = "bad@key".into();
        assert!(e.finished().is_err());
        e.draft.key = "ok".into();
        e.draft.code = " ".into();
        assert_eq!(e.finished().unwrap_err(), "Series and code are required.");
    }

    type State = (CataloguePanel, Shared);

    /// The whole tab on `catalogue`, connected to a fake database whose requests come out of
    /// the receiver when `online`.
    fn panel_harness(
        catalogue: Catalogue,
        online: bool,
    ) -> (egui_kittest::Harness<'static, State>, Receiver<Request>) {
        let (requests, sent) = std::sync::mpsc::channel();
        let mut shared = Shared::new(catalogue);
        if online {
            shared.db = Some(Db::new(requests, std::sync::mpsc::channel().1));
        }
        let mut h = theme::harness(
            Vec2::new(1200.0, 800.0),
            |ui, (panel, shared): &mut State| {
                panel.handle_dropped_files(ui.ctx(), shared);
                panel.ui(ui, shared);
            },
            (CataloguePanel::default(), shared),
        );
        h.run();
        (h, sent)
    }

    #[test]
    fn clicking_a_row_selects_it_and_shows_its_detail() {
        let (mut h, _sent) = panel_harness(catalogue(), false);
        assert!(h.query_by_label_contains("Select a valve type").is_some());
        assert!(h.query_by_label("vt-demo-4-2@1").is_none());
        // Rows read as selectable, named by series and code.
        let row = |h: &egui_kittest::Harness<'static, State>| {
            h.get_by_role_and_label(Role::Button, "DEMO-06 4-2-S, version 1")
                .accesskit_node()
                .toggled()
        };
        assert_eq!(row(&h), Some(Toggled::False));
        h.get_by_role_and_label(Role::Button, "DEMO-06 4-2-S, version 1")
            .click();
        h.run();
        assert_eq!(row(&h), Some(Toggled::True));
        assert_eq!(
            h.get_by_role_and_label(Role::Button, "DEMO-CV 1, version 1")
                .accesskit_node()
                .toggled(),
            Some(Toggled::False)
        );
        // The detail: its title, key@version, symbol, mounting and parts.
        assert!(
            h.query_by_role_and_label(Role::Label, "DEMO-06 4-2-S")
                .is_some()
        );
        assert!(h.query_by_label("vt-demo-4-2@1").is_some());
        assert!(h.query_by_label("sym-4-2-spring-return@1").is_some());
        assert!(h.query_by_label("How Hydra draws it").is_some());
        assert!(h.query_by_label("PARTS (2)").is_some());
        assert!(h.query_by_label("DEMO-06-4-2-S-24DC").is_some());
        // Offline, nothing can be saved or attached, and the detail says why.
        assert!(
            h.query_by_label("Datasheets need the database connection.")
                .is_some()
        );
        assert!(
            h.get_by_label("New valve type")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            h.get_by_label("Edit as new version")
                .accesskit_node()
                .is_disabled()
        );
    }

    #[test]
    fn the_list_marks_missing_symbols_older_versions_and_drafts() {
        let mut c = catalogue();
        let draft = c
            .valve_types
            .iter_mut()
            .find(|v| v.key == "vt-demo-check")
            .unwrap();
        draft.symbol = None;
        let broken = c
            .valve_types
            .iter_mut()
            .find(|v| v.key == "vt-demo-relief")
            .unwrap();
        broken.symbol = Some(CatalogueRef::new("sym-gone", 1));
        let (mut h, _sent) = panel_harness(c, false);
        assert!(h.query_by_label("Draft, no symbol").is_some());
        assert!(h.query_by_label("Symbol missing").is_some());
        // Only one version of each type shows until asked.
        assert!(h.query_by_label("Version 2 is newer").is_none());
        h.get_by_label("All versions").click();
        h.run();
        assert!(h.query_by_label("Version 2 is newer").is_some());
        // Normal valve types say nothing.
        let marks = ["Draft, no symbol", "Symbol missing", "Version 2 is newer"];
        let said = marks
            .iter()
            .filter(|m| h.query_by_label(m).is_some())
            .count();
        assert_eq!(said, 3);
        assert_eq!(h.query_all_by_label_contains("newer").count(), 1);
    }

    #[test]
    fn row_status_puts_a_missing_symbol_before_a_newer_version_before_a_draft() {
        let c = catalogue();
        let mut v = c.valve_types[0].clone();
        assert_eq!(row_status(&v, 1, &c), None);
        v.symbol = None;
        assert_eq!(
            row_status(&v, 1, &c),
            Some((Mark::Warning, "Draft, no symbol".into()))
        );
        assert_eq!(
            row_status(&v, 2, &c),
            Some((Mark::Warning, "Version 2 is newer".into()))
        );
        v.symbol = Some(CatalogueRef::new("sym-gone", 1));
        assert_eq!(
            row_status(&v, 2, &c),
            Some((Mark::Error, "Symbol missing".into()))
        );
    }

    #[test]
    fn the_detail_says_what_is_wrong_and_places_the_latest_version() {
        let mut c = catalogue();
        c.valve_types
            .iter_mut()
            .find(|v| v.key == "vt-demo-check")
            .unwrap()
            .symbol = None;
        let (mut h, _sent) = panel_harness(c, false);
        // An older version: the newer one is named, and placing uses it.
        h.state_mut().0.selected = Some(CatalogueRef::new("vt-rpe3-06-z11", 1));
        h.run();
        assert!(
            h.query_by_label("A newer version exists: vt-rpe3-06-z11@2")
                .is_some()
        );
        assert!(h.query_by_label("Edit latest as new version").is_some());
        h.get_by_label("Place in schematic").click();
        h.run();
        let latest = CatalogueRef::new("vt-rpe3-06-z11", 2);
        assert_eq!(h.state().1.place, Some(latest));
        assert_eq!(
            h.state().1.notice.as_deref(),
            Some("vt-rpe3-06-z11@2 will be placed when you open the Schematic tab.")
        );

        // A draft can't be placed, and says so.
        h.state_mut().1.place = None;
        h.state_mut().0.selected = Some(CatalogueRef::new("vt-demo-check", 1));
        h.run();
        assert!(h.query_by_label_contains("Draft: no symbol yet").is_some());
        assert!(h.query_by_label("No symbol").is_some());
        let place = h.get_by_label("Place in schematic");
        assert!(place.accesskit_node().is_disabled());
        place.click();
        h.run();
        assert_eq!(h.state().1.place, None);

        // A symbol the catalogue lacks is named.
        h.state_mut().1.catalogue.valve_types[1].symbol = Some(CatalogueRef::new("sym-gone", 1));
        let broken = h.state().1.catalogue.valve_types[1].reference();
        h.state_mut().0.selected = Some(broken);
        h.run();
        assert!(
            h.query_by_label_contains("Symbol sym-gone@1 is missing")
                .is_some()
        );
        assert!(
            h.get_by_label("Place in schematic")
                .accesskit_node()
                .is_disabled()
        );
    }

    #[test]
    fn a_new_version_is_checked_then_sent_on_the_latest() {
        let (mut h, sent) = panel_harness(catalogue(), true);
        h.state_mut().0.selected = Some(CatalogueRef::new("vt-rpe3-06-z11", 1));
        h.run();
        h.get_by_label("Edit latest as new version").click();
        h.run();
        assert!(h.query_by_label("New version of vt-rpe3-06-z11").is_some());
        // The fields are labelled, so screen readers name them.
        let code = h.get_by_role_and_label(Role::TextInput, "Code");
        assert_eq!(code.value().as_deref(), Some("Z11-new"));

        // An empty code is refused in plain words, and nothing is sent.
        h.state_mut().0.editor.as_mut().unwrap().draft.code = " ".into();
        h.run();
        h.get_by_label("Save as new version").click();
        h.run();
        assert!(h.query_by_label("Series and code are required.").is_some());
        assert!(sent.try_recv().is_err());

        h.state_mut().0.editor.as_mut().unwrap().draft.code = " Z11-B ".into();
        h.run();
        h.get_by_label("Save as new version").click();
        // A spinner turns while it saves.
        h.run_steps(2);
        let Ok(Request::SaveValveType(v)) = sent.try_recv() else {
            panic!("expected a save")
        };
        // Built on the latest version: the database inserts version 3, never changing 2.
        assert_eq!(v.reference(), CatalogueRef::new("vt-rpe3-06-z11", 2));
        assert_eq!(v.code, "Z11-B");
        assert!(h.query_by_label("Series and code are required.").is_none());
        // While it saves, it can't be closed or saved again.
        assert!(h.get_by_label("Cancel").accesskit_node().is_disabled());
        assert!(
            h.get_by_label("Save as new version")
                .accesskit_node()
                .is_disabled()
        );
        h.state_mut()
            .0
            .saved(CatalogueRef::new("vt-rpe3-06-z11", 3));
        h.run();
        assert!(h.query_by_label("Save as new version").is_none());

        // Cancel closes an editor that isn't saving.
        h.get_by_label("New valve type").click();
        h.run();
        assert!(h.query_by_label("Save as new version").is_some());
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.query_by_label("Save as new version").is_none());
    }

    #[test]
    fn a_dragged_file_shows_the_drop_hint_on_an_ink_scrim() {
        let (mut h, _sent) = panel_harness(catalogue(), false);
        let hint = |h: &egui_kittest::Harness<'static, State>| {
            h.output().shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == DROP_HINT => {
                    Some(t.galley.job.sections[0].format.clone())
                }
                _ => None,
            })
        };
        assert!(hint(&h).is_none());
        h.input_mut()
            .hovered_files
            .push(egui::HoveredFile::default());
        h.step();
        let format = hint(&h).expect("the drop hint");
        assert_eq!(format.color, LIGHT.surface);
        assert_eq!(
            format.font_id,
            egui::TextStyle::Heading.resolve(&h.ctx.global_style())
        );
        let scrim = LIGHT.ink.gamma_multiply(0.8);
        assert!(h.output().shapes.iter().any(|s| matches!(
            &s.shape,
            egui::Shape::Rect(r) if r.fill == scrim
        )));
        // Offline, a dropped file is refused with the reason.
        #[derive(Debug)]
        struct Pdf;
        impl egui::DroppedFile for Pdf {
            fn path(&self) -> &std::path::Path {
                std::path::Path::new("a.pdf")
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                Ok(b"%PDF-1.4".to_vec())
            }
        }
        // The pointer lets go: the file is no longer hovering, it is dropped.
        h.input_mut().hovered_files.clear();
        h.input_mut().dropped_files.push(std::sync::Arc::new(Pdf));
        h.step();
        assert!(hint(&h).is_none());
        assert_eq!(
            h.state().1.error.as_deref(),
            Some("Importing and attaching need the database connection.")
        );
    }

    #[test]
    fn part_options_read_as_words() {
        let options = serde_json::json!({"voltage": "24 V DC", "max_flow_lpm": 80});
        assert_eq!(
            options_text(&options),
            "max_flow_lpm: 80 · voltage: 24 V DC"
        );
        assert_eq!(options_text(&serde_json::Value::Null), "");
        assert_eq!(options_text(&serde_json::json!(["a"])), r#"["a"]"#);
    }

    #[test]
    fn every_row_is_as_tall_as_the_list_assumes() {
        // The list lays out only the rows in view, so a taller row would shift the rest.
        // Text too long for its column is cut short rather than wrapped onto a second line.
        let mut c = catalogue();
        let mut long = c.valve_types[1].clone();
        long.key = "vt-demo-long".into();
        long.series = "DEMO-SERIES-WITH-A-VERY-LONG-NAME".into();
        long.code = "CODE-THAT-GOES-ON-AND-ON".into();
        long.mounting = Some("A MOUNTING PATTERN NAMED AT GREAT LENGTH".into());
        long.manufacturer = "long".into();
        c.manufacturers.push(hydra_core::Manufacturer {
            key: "long".into(),
            name: "A manufacturer whose name is far longer than its column could ever hold".into(),
        });
        c.valve_types.push(long);
        let (mut h, _sent) = panel_harness(c, false);
        h.get_by_label("All versions").click();
        h.run();
        let tops: Vec<f64> = h
            .query_all_by_role(Role::Button)
            .filter_map(|n| {
                let node = n.accesskit_node();
                // The rows, which are selectable: they say whether they are selected.
                node.toggled().map(|_| node.raw_bounds().unwrap())
            })
            .map(|b| b.y0)
            .collect();
        assert_eq!(tops.len(), 14);
        let body = egui::TextStyle::Body.resolve(&h.ctx.global_style());
        let height = h.ctx.fonts_mut(|f| f.row_height(&body)) + ROW_PADDING;
        // A row with a status mark among them, at the top.
        assert!(h.query_by_label("Version 2 is newer").is_some());
        for pair in tops.windows(2) {
            assert!(
                ((pair[1] - pair[0]) as f32 - height).abs() < 0.5,
                "{tops:?} vs {height}"
            );
        }
    }

    #[test]
    fn the_detail_panel_and_the_editor_keep_their_width() {
        // A box drawn wider than the room it was given grows a resizable panel or window a
        // little every frame.
        let (mut h, _sent) = panel_harness(catalogue(), true);
        h.state_mut().0.selected = Some(CatalogueRef::new("vt-demo-4-2", 1));
        h.run();
        h.get_by_label("Edit as new version").click();
        h.run();
        let measure = |h: &egui_kittest::Harness<'static, State>| {
            let heading = h.get_by_role_and_label(Role::Label, "DEMO-06 4-2-S").rect();
            let field = h.get_by_role_and_label(Role::TextInput, "Code").rect();
            (heading.left(), field.width())
        };
        let before = measure(&h);
        for _ in 0..20 {
            h.step();
        }
        assert_eq!(measure(&h), before);
    }

    #[test]
    fn place_refusal_says_why_in_words() {
        let mut c = catalogue();
        let r = CatalogueRef::new("vt-demo-4-2", 1);
        assert_eq!(place_refusal(&c, &r), None);
        let t = c
            .valve_types
            .iter_mut()
            .find(|v| v.key == "vt-demo-4-2")
            .unwrap();
        t.symbol = None;
        assert_eq!(
            place_refusal(&c, &r).unwrap(),
            "vt-demo-4-2@1 has no symbol, so it can't be drawn. Give it one in a new version."
        );
        let t = c
            .valve_types
            .iter_mut()
            .find(|v| v.key == "vt-demo-4-2")
            .unwrap();
        t.symbol = Some(CatalogueRef::new("sym-gone", 1));
        assert_eq!(
            place_refusal(&c, &r).unwrap(),
            "Symbol sym-gone@1 of vt-demo-4-2@1 is missing from the catalogue, so it can't be \
             drawn."
        );
    }

    #[test]
    fn saving_without_the_database_says_so_and_keeps_the_edits() {
        let (mut h, sent) = panel_harness(catalogue(), true);
        h.state_mut().0.selected = Some(CatalogueRef::new("vt-demo-4-2", 1));
        h.run();
        h.get_by_label("Edit as new version").click();
        h.run();
        h.state_mut().0.editor.as_mut().unwrap().draft.code = "4-2-X".into();
        h.state_mut().1.db = None;
        h.run();
        h.get_by_label("Save as new version").click();
        h.run();
        assert!(
            h.query_by_label_contains("Saving needs the database connection")
                .is_some()
        );
        assert!(sent.try_recv().is_err());
        assert_eq!(h.state().0.editor.as_ref().unwrap().draft.code, "4-2-X");
    }

    #[test]
    fn an_empty_catalogue_says_so_and_a_search_says_nothing_matched() {
        let (mut h, _sent) = panel_harness(Catalogue::default(), false);
        assert!(
            h.query_by_label("The catalogue has no valve types.")
                .is_some()
        );
        h.state_mut().0.search = "nothing".into();
        h.run();
        assert!(
            h.query_by_label("No valve types match the search.")
                .is_some()
        );
    }

    #[test]
    fn the_last_row_of_a_long_list_scrolls_into_view_and_selects() {
        let mut c = catalogue();
        for i in 0..300 {
            let mut v = c.valve_types[1].clone();
            v.key = format!("vt-demo-long-{i:03}");
            v.series = "DEMO-LONG".into();
            v.code = format!("{i:03}");
            c.valve_types.push(v);
        }
        let (mut h, _sent) = panel_harness(c, false);
        let last = "DEMO-TH 1, version 1";
        // Only the rows in view are laid out.
        assert!(h.query_by_label(last).is_none());
        let input = h.input_mut();
        input
            .events
            .push(egui::Event::PointerMoved(egui::pos2(300.0, 400.0)));
        input.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -100_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // The wheel scrolls smoothly over a few frames.
        h.run_steps(30);
        assert!(h.query_by_label("RPE3-06 Z11, version 2").is_none());
        h.get_by_role_and_label(Role::Button, last).click();
        h.run();
        assert_eq!(
            h.state().0.selected,
            Some(CatalogueRef::new("vt-demo-throttle", 1))
        );
        let row = h.get_by_role_and_label(Role::Button, last);
        assert_eq!(row.accesskit_node().toggled(), Some(Toggled::True));
        assert!(
            h.query_by_role_and_label(Role::Label, "DEMO-TH 1")
                .is_some()
        );
    }

    #[test]
    fn columns_fill_the_list_and_shrink_alike_when_narrow() {
        let gaps = 26.0 + 12.0 * 6.0;
        let wide = column_widths(1200.0);
        assert_eq!(wide[0], 100.0);
        assert!((wide.iter().sum::<f32>() + gaps - 1200.0).abs() < 0.01);
        let narrow = column_widths(500.0);
        assert!((narrow.iter().sum::<f32>() + gaps - 500.0).abs() < 0.01);
        assert!((narrow[0] / narrow[1] - 100.0 / 90.0).abs() < 0.01);
        assert!(narrow[2] > 0.0, "the manufacturer keeps a share");
    }
}
