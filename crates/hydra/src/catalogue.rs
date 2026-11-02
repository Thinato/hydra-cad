//! Catalogue tab: search and browse valve types, edit them as new versions, attach datasheets.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Sense, Vec2};
use egui_extras::{Column, TableBuilder};
use hydra_core::{Catalogue, CatalogueRef, SymbolDef, ValveType};
use hydra_symbols::render_symbol;

use crate::db::{Db, Reply, Request};
use crate::schematic::draw_fitted;

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

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search manufacturer, series, code, mounting")
                    .desired_width(280.0),
            );
            ui.checkbox(&mut self.all_versions, "All versions");
            if shared.db.is_some()
                && ui
                    .add_enabled(!self.saving(), egui::Button::new("New valve type"))
                    .clicked()
            {
                self.editor = Some(Editor::new(new_valve_type(&shared.catalogue), true));
            }
        });

        egui::Panel::bottom("catalogue-detail")
            .default_size(280.0)
            .min_size(120.0)
            .resizable(true)
            .show(ui, |ui| self.detail(ui, shared));
        // Bound the table to what the panel left over; it would otherwise draw its column
        // lines across the panel.
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            ui.set_clip_rect(rest);
            self.table(ui, &shared.catalogue, &shared.datasheets);
        });
        self.editor_window(ui.ctx(), shared);
    }

    /// An editor is waiting on a save; opening another would lose its reply.
    fn saving(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.saving)
    }

    fn table(
        &mut self,
        ui: &mut egui::Ui,
        catalogue: &Catalogue,
        datasheets: &HashMap<CatalogueRef, String>,
    ) {
        let rows = visible_rows(catalogue, &self.search, self.all_versions);
        let mut clicked = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            // Manufacturer names can be long; that column takes the slack and clips.
            .column(Column::remainder().at_least(100.0).clip(true))
            .columns(Column::auto().at_least(50.0), 6)
            .header(20.0, |mut header| {
                for title in [
                    "Manufacturer",
                    "Series",
                    "Code",
                    "Version",
                    "Mounting",
                    "Symbol",
                    "Datasheet",
                ] {
                    header.col(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                    });
                }
            })
            .body(|mut body| {
                for v in rows {
                    let r = v.reference();
                    body.row(20.0, |mut row| {
                        row.set_selected(self.selected.as_ref() == Some(&r));
                        let symbol = match &v.symbol {
                            Some(s) => catalogue.symbol_def(s).map_or("missing", kind_name),
                            None => "draft",
                        };
                        let cells = [
                            manufacturer_name(catalogue, &v.manufacturer),
                            &v.series,
                            &v.code,
                            &v.version.to_string(),
                            v.mounting.as_deref().unwrap_or(""),
                            symbol,
                            if datasheets.contains_key(&r) {
                                "PDF"
                            } else {
                                ""
                            },
                        ];
                        for text in cells {
                            row.col(|ui| {
                                ui.add(egui::Label::new(text).truncate());
                            });
                        }
                        if row.response().clicked() {
                            clicked = Some(r);
                        }
                    });
                }
            });
        if clicked.is_some() {
            self.selected = clicked;
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let Some(v) = self
            .selected
            .as_ref()
            .and_then(|r| shared.catalogue.valve_type(r))
            .cloned()
        else {
            ui.weak("Select a valve type.");
            return;
        };
        let primitives = v
            .symbol
            .as_ref()
            .and_then(|s| shared.catalogue.symbol_def(s))
            .map(|def| render_symbol(def).primitives);
        let symbol = v.symbol.as_ref().map(|s| (s, primitives.is_some()));
        ui.horizontal_top(|ui| {
            let side = Vec2::new(320.0, ui.available_height().clamp(120.0, 240.0));
            match &primitives {
                Some(p) => draw_fitted(ui, side, p),
                None => {
                    ui.allocate_exact_size(side, Sense::hover());
                }
            }
            ui.vertical(|ui| self.fields(ui, &v, symbol, shared));
        });
    }

    fn fields(
        &mut self,
        ui: &mut egui::Ui,
        v: &ValveType,
        symbol: Option<(&CatalogueRef, bool)>,
        shared: &mut Shared,
    ) {
        ui.heading(format!("{} {}", v.series, v.code));
        ui.label(manufacturer_name(&shared.catalogue, &v.manufacturer));
        ui.monospace(v.reference().to_string());
        let latest = shared
            .catalogue
            .valve_types
            .iter()
            .filter(|o| o.key == v.key)
            .map(|o| o.version)
            .max()
            .unwrap_or(v.version);
        if latest > v.version {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("A newer version exists: {}@{latest}", v.key),
            );
        }
        if let Some(m) = &v.mounting {
            ui.label(format!("Mounting: {m}"));
        }
        match symbol {
            Some((s, true)) => {
                ui.label(format!("Symbol: {s}"));
            }
            Some((s, false)) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Symbol {s} is missing"),
                );
            }
            None => {
                ui.weak("Draft: no symbol yet.");
            }
        }
        if !v.params.is_null() {
            ui.monospace(serde_json::to_string(&v.params).unwrap_or_default());
        }
        let r = v.reference();
        let parts: Vec<_> = shared.catalogue.parts_of(&r).collect();
        if !parts.is_empty() {
            ui.label(format!("Parts ({})", parts.len()));
            for p in parts {
                let options = if p.options.is_null() {
                    String::new()
                } else {
                    serde_json::to_string(&p.options).unwrap_or_default()
                };
                ui.horizontal(|ui| {
                    ui.monospace(&p.part_number);
                    ui.weak(options);
                });
            }
        }
        ui.separator();
        match (shared.datasheets.get(&v.reference()), &shared.db) {
            (Some(sha256), Some(db)) => {
                if ui.button("Open datasheet").clicked() {
                    db.send(Request::OpenAsset {
                        sha256: sha256.clone(),
                        extension: "pdf",
                    });
                }
            }
            (None, Some(_)) => {
                ui.weak("No datasheet. Drop a PDF on this window to attach it.");
            }
            (_, None) => {
                ui.weak("Datasheets need the database connection.");
            }
        }
        // New parts use the latest version, like the editor's own valve list.
        let newest = CatalogueRef::new(&v.key, latest);
        if shared.catalogue.symbol_of(&newest).is_some()
            && ui.button("Place in schematic").clicked()
        {
            shared.notice = Some(format!(
                "{newest} will be placed when you open the Schematic tab."
            ));
            shared.place = Some(newest);
        }
        // A new version always builds on the latest one, whichever version is on screen.
        let label = if latest > v.version {
            "Edit latest as new version"
        } else {
            "Edit as new version"
        };
        if shared.db.is_some()
            && ui
                .add_enabled(!self.saving(), egui::Button::new(label))
                .clicked()
            && let Some(base) = shared
                .catalogue
                .valve_type(&CatalogueRef::new(&v.key, latest))
        {
            self.editor = Some(Editor::new(base.clone(), false));
        }
    }

    fn editor_window(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.is_new {
            "New valve type".to_string()
        } else {
            format!("New version of {}", editor.draft.key)
        };
        let mut open = true;
        // Closing is ignored while saving: the reply must find the editor that sent it.
        let saving = editor.saving;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| editor.ui(ui, shared));
        if !open && !saving {
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
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a PDF to attach it to the selected valve type,\nor a catalogue JSON file to import it",
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
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

    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let catalogue = &shared.catalogue;
        egui::Grid::new("valve-type-editor")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Key");
                if self.is_new {
                    ui.text_edit_singleline(&mut self.draft.key);
                } else {
                    ui.monospace(&self.draft.key);
                }
                ui.end_row();

                ui.label("Manufacturer");
                egui::ComboBox::from_id_salt("manufacturer")
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
                ui.end_row();

                ui.label("Series");
                ui.text_edit_singleline(&mut self.draft.series);
                ui.end_row();
                ui.label("Code");
                ui.text_edit_singleline(&mut self.draft.code);
                ui.end_row();
                ui.label("Mounting");
                ui.add(
                    egui::TextEdit::singleline(&mut self.mounting).hint_text("e.g. ISO 4401-03"),
                );
                ui.end_row();

                ui.label("Symbol");
                let label = |s: &Option<CatalogueRef>| match s {
                    Some(r) => r.to_string(),
                    None => "None (draft)".to_string(),
                };
                egui::ComboBox::from_id_salt("symbol")
                    .selected_text(label(&self.draft.symbol))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.draft.symbol, None, label(&None));
                        for s in &catalogue.symbol_defs {
                            let text = format!("{} ({})", s.reference(), kind_name(&s.definition));
                            ui.selectable_value(&mut self.draft.symbol, Some(s.reference()), text);
                        }
                    });
                ui.end_row();
            });
        if let Some(def) = self
            .draft
            .symbol
            .as_ref()
            .and_then(|s| catalogue.symbol_def(s))
        {
            let width = ui.available_width();
            draw_fitted(ui, Vec2::new(width, 140.0), &render_symbol(def).primitives);
        }
        ui.label("Parameters (JSON, optional)");
        ui.add(
            egui::TextEdit::multiline(&mut self.params)
                .code_editor()
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.horizontal(|ui| {
            let save = ui.add_enabled(!self.saving, egui::Button::new("Save as new version"));
            if self.saving {
                ui.spinner();
            }
            if save.clicked() {
                match self.finished() {
                    Ok(v) => {
                        if let Some(db) = &shared.db {
                            self.saving = true;
                            self.error = None;
                            db.send(Request::SaveValveType(v));
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        });
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

/// Valve types matching the search, latest version of each key unless `all_versions`.
fn visible_rows<'a>(
    catalogue: &'a Catalogue,
    search: &str,
    all_versions: bool,
) -> Vec<&'a ValveType> {
    let mut latest: HashMap<&str, u32> = HashMap::new();
    for v in &catalogue.valve_types {
        let e = latest.entry(&v.key).or_default();
        *e = (*e).max(v.version);
    }
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
    use super::*;

    fn catalogue() -> Catalogue {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
        let mut c: Catalogue =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut v2 = c.valve_types[0].clone();
        v2.version = 2;
        v2.code = "4-3-C-new".into();
        c.valve_types.push(v2);
        c
    }

    #[test]
    fn search_shows_latest_versions_unless_asked() {
        let c = catalogue();
        let z11: Vec<_> = visible_rows(&c, "4-3-c", false)
            .iter()
            .map(|v| v.reference())
            .collect();
        assert_eq!(z11, [CatalogueRef::new("vt-demo-4-3-closed", 2)]);
        assert_eq!(visible_rows(&c, "4-3-C", true).len(), 2);
        // Manufacturer names are searchable, not just keys.
        let latest = c.valve_types.len() - 1;
        assert_eq!(visible_rows(&c, "fixture data", false).len(), latest);
        assert_eq!(visible_rows(&c, "", false).len(), c.valve_types.len() - 1);
        assert!(visible_rows(&c, "no such valve", true).is_empty());
    }

    #[test]
    fn dropped_files_attach_or_import() {
        use std::path::Path;
        let z11 = CatalogueRef::new("vt-demo-4-3-closed", 2);
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
}
