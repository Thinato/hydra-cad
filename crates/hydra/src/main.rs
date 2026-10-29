mod catalogue;
mod db;
mod mesh;
mod schematic;
mod viewer3d;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use hydra_core::{Catalogue, Circuit};

use catalogue::{CataloguePanel, Shared};
use db::{Db, Reply};

// ponytail: dev paths baked in at compile time; models move to the asset cache in phase 4.
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const PLACEHOLDER_VALVE: &str = "assets/valve-cetop3-placeholder.glb";
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
    /// Load errors are shown in the tab instead of stopping the app.
    schematic: Result<schematic::SchematicView, String>,
    catalogue: CataloguePanel,
    viewer: viewer3d::Viewer3d,
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
            Tab::Schematic => match &mut self.schematic {
                Ok(view) => view.ui(ui),
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                }
            },
            Tab::Catalogue => self.catalogue.ui(ui, &mut self.shared),
            Tab::View3d => self.viewer.ui(ui),
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
                    self.shared.error = None;
                    if let Ok(view) = &mut self.schematic {
                        view.set_catalogue(&self.shared.catalogue);
                    }
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
                        "Imported {} manufacturers, {} symbols, {} valve types and {} parts ({} rows unchanged).",
                        r.manufacturers_added,
                        r.symbol_defs_added,
                        r.valve_types_added,
                        r.parts_added,
                        r.unchanged
                    ))
                }
                Reply::SaveFailed(e) => {
                    if !self.catalogue.save_failed(&e) {
                        self.shared.error = Some(e);
                    }
                }
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
            let mesh =
                mesh::load_glb(&std::fs::read(PathBuf::from(ROOT).join(PLACEHOLDER_VALVE))?)?;
            let mut shared = Shared {
                catalogue: Catalogue::default(),
                datasheets: HashMap::new(),
                db: None,
                error: None,
                notice: None,
            };
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
            let schematic = read(&circuit_path)
                .and_then(|t| Ok(Circuit::from_json(&t)?))
                .map(|c| schematic::SchematicView::new(c, &shared.catalogue))
                .map_err(|e| {
                    tracing::error!("could not load circuit: {e:#}");
                    format!("Could not load the circuit: {e:#}")
                });

            let mut dock = DockState::new(vec![Tab::Schematic, Tab::Catalogue]);
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.6, vec![Tab::View3d]);
            Ok(Box::new(HydraApp {
                dock,
                panels: Panels {
                    shared,
                    schematic,
                    catalogue: CataloguePanel::default(),
                    viewer: viewer3d::Viewer3d::new(cc, &mesh)?,
                },
            }))
        }),
    )
}
