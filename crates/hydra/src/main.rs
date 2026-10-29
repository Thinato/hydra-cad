mod catalogue;
mod db;
mod editor;
mod models;
mod schematic;
mod viewer3d;

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use hydra_core::{Catalogue, Circuit};

use catalogue::{CataloguePanel, Shared};
use db::{Db, Reply};

// ponytail: dev paths baked in at compile time; installers set real defaults in phase 6.
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const DEMO_CIRCUIT: &str = "fixtures/circuits/demo-manifold.json";
const CATALOGUE: &str = "fixtures/catalogue.json";
/// Postgres URL of the shared database; without it the app runs read-only on a fixture catalogue.
const DATABASE_URL_VAR: &str = "HYDRA_DATABASE_URL";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Schematic,
    Catalogue,
    View3d,
}

struct Panels {
    shared: Shared,
    schematic: editor::SchematicEditor,
    catalogue: CataloguePanel,
    models: models::ModelPanel,
}

impl TabViewer for Panels {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab as u8)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Schematic => "Schematic".into(),
            Tab::Catalogue => "Catalogue".into(),
            Tab::View3d => "3D".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Schematic => self.schematic.ui(ui, &mut self.shared),
            Tab::Catalogue => self.catalogue.ui(ui, &mut self.shared),
            Tab::View3d => self.models.ui(ui, &self.shared, &mut self.schematic),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }
}

impl Panels {
    /// Apply what the database worker sent since the last frame.
    fn poll_db(&mut self) {
        let replies = self.shared.db.as_ref().map(Db::poll).unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Catalogue(loaded) => {
                    self.shared.catalogue = loaded.catalogue;
                    self.shared.datasheets = loaded.datasheets;
                    self.shared.valve_models = loaded.valve_models;
                    self.shared.block_models = loaded.block_models;
                    self.models.retry_failed();
                    self.shared.error = None;
                }
                Reply::Saved(r) => {
                    self.shared.notice = Some(format!("Saved {r}."));
                    self.catalogue.saved(r);
                }
                Reply::Attached(r) => {
                    self.shared.notice = Some(format!("Datasheet attached to {r}."))
                }
                Reply::Imported(r) => {
                    self.shared.notice = Some(format!(
                        "Imported {} manufacturers, {} symbols, {} valve types, {} parts and {} blocks ({} rows unchanged).",
                        r.manufacturers_added,
                        r.symbol_defs_added,
                        r.valve_types_added,
                        r.parts_added,
                        r.blocks_added,
                        r.unchanged
                    ))
                }
                Reply::SaveFailed(e) => {
                    if !self.catalogue.save_failed(&e) {
                        self.shared.error = Some(e);
                    }
                }
                Reply::Circuits(list) => self.schematic.circuits(list),
                Reply::Opened(opened) => self.schematic.opened(opened),
                Reply::CircuitSaved { id, revision } => self.schematic.circuit_saved(id, revision),
                Reply::Conflict { current } => self.schematic.conflict(current),
                Reply::CircuitSaveFailed(e) => self.schematic.save_failed(e),
                Reply::CircuitFailed(e) => self.schematic.circuit_failed(e),
                Reply::Revisions(list) => self.schematic.revisions(list),
                Reply::RevisionPreview(n, c) => {
                    self.schematic
                        .revision_preview(n, &c, &self.shared.catalogue)
                }
                Reply::Model(sha, mesh) => self.models.loaded(sha, mesh),
                Reply::Error(e) => self.shared.error = Some(e),
            }
        }
    }
}

struct HydraApp {
    dock: DockState<Tab>,
    panels: Panels,
}

impl eframe::App for HydraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.panels.poll_db();
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.panels.schematic.may_close()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let panels = &mut self.panels;
        panels
            .catalogue
            .handle_dropped_files(ui.ctx(), &mut panels.shared);
        // Status line above the tabs, so database errors show whichever tab is open.
        let (error, notice) = (&panels.shared.error, &panels.shared.notice);
        if error.is_some() || notice.is_some() {
            ui.horizontal(|ui| {
                if let Some(e) = error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if let Some(n) = notice {
                    ui.weak(n);
                }
            });
        }
        DockArea::new(&mut self.dock)
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut self.panels);
    }
}

fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// `hydra [CIRCUIT.json] [CATALOGUE.json]`, defaulting to the demo fixtures.
///
/// With `HYDRA_DATABASE_URL` set, the catalogue comes from Postgres and CATALOGUE is ignored.
fn main() -> eframe::Result {
    tracing_subscriber::fmt::init();
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let circuit_path = args
        .next()
        .unwrap_or_else(|| PathBuf::from(ROOT).join(DEMO_CIRCUIT));
    let catalogue_path = args
        .next()
        .unwrap_or_else(|| PathBuf::from(ROOT).join(CATALOGUE));
    let database_url = std::env::var(DATABASE_URL_VAR).ok();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: viewer3d::DEPTH_BUFFER_BITS,
        multisampling: viewer3d::MSAA_SAMPLES,
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Hydra",
        options,
        Box::new(move |cc| {
            let mut shared = Shared::new(Catalogue::default());
            match database_url {
                Some(url) => shared.db = Some(Db::connect(url, cc.egui_ctx.clone())),
                None => match read(&catalogue_path)
                    .and_then(|t| Ok(serde_json::from_str::<Catalogue>(&t)?))
                {
                    Ok(c) => {
                        shared.catalogue = c;
                        shared.notice = Some(format!(
                            "Offline: read-only catalogue from {}. Set {DATABASE_URL_VAR} to use the shared database.",
                            catalogue_path.display()
                        ));
                    }
                    Err(e) => shared.error = Some(format!("Could not load the catalogue: {e:#}")),
                },
            }
            // The file opens as a new, unsaved circuit; Save puts it in the database.
            let circuit = read(&circuit_path)
                .and_then(|t| Ok(Circuit::from_json(&t)?))
                .unwrap_or_else(|e| {
                    tracing::error!("could not load circuit: {e:#}");
                    shared.error = Some(format!("Could not load the circuit: {e:#}"));
                    editor::empty_circuit()
                });
            let schematic = editor::SchematicEditor::new(circuit, None);

            let mut dock = DockState::new(vec![Tab::Schematic, Tab::Catalogue]);
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.6, vec![Tab::View3d]);
            Ok(Box::new(HydraApp {
                dock,
                panels: Panels {
                    shared,
                    schematic,
                    catalogue: CataloguePanel::default(),
                    models: models::ModelPanel::new(
                        viewer3d::Viewer3d::new(cc)
                            .inspect_err(|e| tracing::error!("3D view: {e:#}"))
                            .ok(),
                    ),
                },
            }))
        }),
    )
}
