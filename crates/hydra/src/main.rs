mod mesh;
mod schematic;
mod viewer3d;

use std::path::PathBuf;

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};

// ponytail: dev paths baked in at compile time; the catalogue moves to Postgres in phase 2
// and models to the asset cache in phase 4.
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const PLACEHOLDER_VALVE: &str = "assets/valve-cetop3-placeholder.glb";
const DEMO_CIRCUIT: &str = "fixtures/circuits/demo-manifold.json";
const CATALOGUE: &str = "fixtures/catalogue.json";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Schematic,
    View3d,
}

struct Panels {
    /// Load errors are shown in the tab instead of stopping the app.
    schematic: Result<schematic::SchematicView, String>,
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
            Tab::View3d => "3D".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match (tab, &mut self.schematic) {
            (Tab::Schematic, Ok(view)) => view.ui(ui),
            (Tab::Schematic, Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
            }
            (Tab::View3d, _) => self.viewer.ui(ui),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }
}

struct HydraApp {
    dock: DockState<Tab>,
    panels: Panels,
}

impl eframe::App for HydraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        DockArea::new(&mut self.dock)
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut self.panels);
    }
}

/// `hydra [CIRCUIT.json] [CATALOGUE.json]`, defaulting to the demo fixtures.
fn main() -> eframe::Result {
    tracing_subscriber::fmt::init();
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let circuit = args
        .next()
        .unwrap_or_else(|| PathBuf::from(ROOT).join(DEMO_CIRCUIT));
    let catalogue = args
        .next()
        .unwrap_or_else(|| PathBuf::from(ROOT).join(CATALOGUE));

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
            let schematic = schematic::SchematicView::load(&circuit, &catalogue).map_err(|e| {
                tracing::error!("could not load circuit: {e:#}");
                format!("Could not load the circuit: {e:#}")
            });
            let mut dock = DockState::new(vec![Tab::Schematic]);
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.55, vec![Tab::View3d]);
            Ok(Box::new(HydraApp {
                dock,
                panels: Panels {
                    schematic,
                    viewer: viewer3d::Viewer3d::new(cc, &mesh)?,
                },
            }))
        }),
    )
}
