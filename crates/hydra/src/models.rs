//! 3D tab: the circuit's manifold block with its valves mounted on their faces, or else the
//! valve selected in the schematic. Models download once and are cached by hash.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Margin, RichText};
use glam::Mat4;
use hydra_core::{Circuit, Command, ComponentKind, Issue, Severity};
use hydra_mesh::Mesh;

use crate::catalogue::Shared;
use crate::db::Request;
use crate::editor::SchematicEditor;
use crate::theme::{self, LIGHT, Mark};
use crate::viewer3d::{Instance, Viewer3d};

/// How to move round the view, for the status bar.
const VIEW_KEYS: &str = "Drag to turn · scroll or pinch to zoom · double-click fits";

/// The colour a model is shaded in: the block light grey, valves darker, and the valve
/// selected in the schematic in the accent, as selection is everywhere.
fn tint(component: Option<&str>, selected: Option<&str>) -> Color32 {
    match component {
        None => LIGHT.border,
        Some(c) if Some(c) == selected => LIGHT.accent,
        Some(_) => LIGHT.border_strong,
    }
}

/// A model to draw: which asset, and where in block coordinates (millimetres, Z up).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub sha256: String,
    /// Column-major.
    pub transform: [[f64; 4]; 4],
    /// The circuit component it shows; `None` for the block.
    pub component: Option<String>,
}

const IDENTITY: [[f64; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A note on what the 3D view can't show, with the mark of its tone: ■ an error, ▲ a warning.
pub type Note = (Mark, String);

/// What the 3D view shows for `circuit`, and notes on what it can't show.
pub fn scene(
    circuit: &Circuit,
    selected: Option<&str>,
    shared: &Shared,
) -> (Vec<Placed>, Vec<Note>) {
    let catalogue = &shared.catalogue;
    let (mut placed, mut notes) = (Vec::new(), Vec::new());
    let valve_model = |component: &str, notes: &mut Vec<Note>| {
        let ComponentKind::Valve(v) = &circuit.component(component)?.kind else {
            return None;
        };
        let Some(valve_type) = &v.valve_type else {
            notes.push((
                Mark::Error,
                format!("{component} is missing from the catalogue."),
            ));
            return None;
        };
        let model = shared.valve_models.get(valve_type).cloned();
        if model.is_none() {
            let name = catalogue
                .valve_type(valve_type)
                .map_or(valve_type.to_string(), |v| {
                    format!("{} {}", v.series, v.code)
                });
            notes.push((
                Mark::Warning,
                format!("{component} ({name}) has no 3D model."),
            ));
        }
        model
    };
    match &circuit.assembly {
        Some(assembly) => {
            let Some(block) = catalogue.block(&assembly.block) else {
                return (placed, notes);
            };
            match shared.block_models.get(&assembly.block) {
                Some(sha) => placed.push(Placed {
                    sha256: sha.clone(),
                    transform: IDENTITY,
                    component: None,
                }),
                None => notes.push((
                    Mark::Warning,
                    format!("Block {} has no 3D model.", block.name),
                )),
            }
            for (component, face) in &assembly.mounts {
                // Unknown faces are validation issues, listed with the others.
                let Some(face) = block.face(face) else {
                    continue;
                };
                let Some(sha256) = valve_model(component, &mut notes) else {
                    continue;
                };
                match face.transform() {
                    Ok(transform) => placed.push(Placed {
                        sha256,
                        transform,
                        component: Some(component.clone()),
                    }),
                    Err(e) => notes.push((Mark::Error, format!("Block {}: {e}", block.name))),
                }
            }
        }
        None => {
            if let Some(id) = selected
                && let Some(sha256) = valve_model(id, &mut notes)
            {
                placed.push(Placed {
                    sha256,
                    transform: IDENTITY,
                    component: Some(id.to_string()),
                });
            }
        }
    }
    (placed, notes)
}

/// Assembly findings from validation, which the 3D tab lists next to the block.
fn is_assembly_issue(issue: &Issue) -> bool {
    matches!(
        issue,
        Issue::UnknownBlock(_)
            | Issue::MountedUnknownComponent { .. }
            | Issue::MountedPort(_)
            | Issue::UnknownFace { .. }
            | Issue::FaceTaken { .. }
            | Issue::PatternMismatch { .. }
            | Issue::Unmounted(_)
            | Issue::MountedOutsideBlock(_)
            | Issue::NothingInBlock
    )
}

/// A model file shown on its own instead of the circuit.
struct Preview {
    name: String,
    /// "W x D x H mm, z a..b".
    size: String,
    mesh: Arc<Mesh>,
    /// For the viewer's mesh cache.
    key: String,
}

impl Preview {
    fn load(name: String, bytes: &[u8]) -> Result<Self, String> {
        let mesh = hydra_mesh::load_glb(bytes)?;
        let (lo, hi) = mesh.bounds();
        let size = format!(
            "{:.1} x {:.1} x {:.1} mm, z {:.1}..{:.1}",
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2],
            lo[2],
            hi[2]
        );
        Ok(Self {
            name,
            size,
            mesh: Arc::new(mesh),
            key: format!("preview:{}", hydra_db::sha256_hex(bytes)),
        })
    }
}

enum Model {
    Loading,
    Ready(Arc<Mesh>),
    Failed(String),
}

pub struct ModelPanel {
    /// `None` without the wgpu renderer.
    viewer: Option<Viewer3d>,
    models: HashMap<String, Model>,
    status: Option<String>,
    preview: Option<Preview>,
}

impl ModelPanel {
    pub fn new(viewer: Option<Viewer3d>) -> Self {
        Self {
            viewer,
            models: HashMap::new(),
            status: None,
            preview: None,
        }
    }

    /// Try models that failed to load again, e.g. after the catalogue was reloaded.
    pub fn retry_failed(&mut self) {
        self.models.retain(|_, m| !matches!(m, Model::Failed(_)));
    }

    /// A model the worker fetched and loaded, or why it couldn't.
    pub fn loaded(&mut self, sha256: String, mesh: Result<Arc<Mesh>, String>) {
        let model = match mesh {
            Ok(m) => Model::Ready(m),
            Err(e) => Model::Failed(e),
        };
        self.models.insert(sha256, model);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &Shared, editor: &mut SchematicEditor) {
        let fill = |color| egui::Frame::new().fill(color);
        let mut close = false;
        egui::Panel::top("3d-toolbar")
            .frame(fill(LIGHT.panel).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = 32.0;
                ui.horizontal_wrapped(|ui| match &self.preview {
                    Some(p) => {
                        close = ui.button("Close preview").clicked();
                        ui.label(
                            RichText::new(&p.name)
                                .family(egui::FontFamily::Name(theme::SEMIBOLD.into())),
                        );
                        ui.label(RichText::new(&p.size).monospace().color(LIGHT.secondary));
                    }
                    None => {
                        let pick = ui.button("Preview a model file…").on_hover_text(
                            "Show a .glb file on its own, with its size, before attaching it",
                        );
                        if pick.clicked() {
                            self.pick_preview();
                        }
                    }
                });
            });
        egui::Panel::bottom("3d-status")
            .frame(fill(LIGHT.chrome).inner_margin(Margin::symmetric(12, 4)))
            .show(ui, |ui| {
                let keys = RichText::new(VIEW_KEYS).small().color(LIGHT.secondary);
                ui.add(egui::Label::new(keys).wrap());
            });
        if let Some(p) = &self.preview {
            let scene = [Instance {
                key: p.key.clone(),
                mesh: p.mesh.clone(),
                transform: Mat4::IDENTITY,
                color: LIGHT.border_strong,
            }];
            self.viewport(ui, &scene, None);
            if close {
                self.preview = None;
            }
            return;
        }
        let (placed, mut notes) = scene(editor.circuit(), editor.selected(), shared);
        if let Some(db) = &shared.db {
            for p in &placed {
                if !self.models.contains_key(&p.sha256) {
                    db.send(Request::LoadModel(p.sha256.clone()));
                    self.models.insert(p.sha256.clone(), Model::Loading);
                }
            }
        }
        for p in &placed {
            if let Some(Model::Failed(e)) = self.models.get(&p.sha256) {
                let what = p.component.as_deref().unwrap_or("The block");
                notes.push((Mark::Error, format!("{what}: {e}")));
            }
        }

        egui::Panel::left("assembly")
            .default_size(240.0)
            .min_size(210.0)
            .max_size(420.0)
            .resizable(true)
            .frame(fill(LIGHT.panel))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.assembly(ui, shared, editor, &notes));
            });

        let mut loading = 0;
        let mut scene = Vec::new();
        for p in &placed {
            match self.models.get(&p.sha256) {
                Some(Model::Ready(mesh)) => scene.push(Instance {
                    key: p.sha256.clone(),
                    mesh: mesh.clone(),
                    transform: Mat4::from_cols_array_2d(&p.transform.map(|c| c.map(|v| v as f32))),
                    color: tint(p.component.as_deref(), editor.selected()),
                }),
                Some(Model::Failed(_)) => {}
                _ => loading += 1,
            }
        }
        let hint = if loading > 0 {
            Some(format!("Loading {loading} model(s)…"))
        } else if placed.is_empty() && notes.is_empty() && editor.circuit().assembly.is_none() {
            // With a block or notes, the panel says why nothing shows.
            Some(
                "Choose a block for the circuit, or select a valve in the schematic, to see it in 3D."
                    .into(),
            )
        } else {
            None
        };
        self.viewport(ui, &scene, hint);
    }

    /// The view filling what is left, with a hint in `secondary` above it when there is one.
    fn viewport(&mut self, ui: &mut egui::Ui, scene: &[Instance], hint: Option<String>) {
        // Without the renderer nothing else can show, so that comes first.
        let hint = match self.viewer {
            None => Some("3D needs the wgpu renderer.".to_string()),
            Some(_) => hint,
        };
        if let Some(hint) = hint {
            let hint = RichText::new(hint).color(LIGHT.secondary);
            egui::Frame::new()
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| ui.add(egui::Label::new(hint).wrap()));
        }
        if let Some(viewer) = self.viewer.as_mut() {
            viewer.ui(ui, scene);
        }
    }

    fn assembly(
        &mut self,
        ui: &mut egui::Ui,
        shared: &Shared,
        editor: &mut SchematicEditor,
        notes: &[Note],
    ) {
        let catalogue = &shared.catalogue;
        let circuit = editor.circuit();
        let current = circuit.assembly.as_ref().map(|a| a.block.clone());
        let mut edit = None;

        let label = |r: &Option<hydra_core::CatalogueRef>| match r {
            None => "None".to_string(),
            Some(r) => catalogue
                .block(r)
                .map_or(r.to_string(), |b| format!("{} (v{})", b.name, b.version)),
        };
        theme::section(ui, |ui| {
            theme::column_title(ui, "BLOCK");
            let mut choice = current.clone();
            egui::ComboBox::from_id_salt("block")
                .selected_text(label(&current))
                .width(ui.available_width())
                .truncate()
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, "None");
                    // The latest version of each block, plus the one in use if it is older.
                    let mut blocks: Vec<_> = catalogue
                        .blocks
                        .iter()
                        .filter(|b| {
                            Some(b.reference()) == current
                                || !catalogue
                                    .blocks
                                    .iter()
                                    .any(|o| o.key == b.key && o.version > b.version)
                        })
                        .collect();
                    blocks.sort_by(|a, b| (&a.name, a.version).cmp(&(&b.name, b.version)));
                    for b in blocks {
                        let r = Some(b.reference());
                        let text = label(&r);
                        ui.selectable_value(&mut choice, r, text);
                    }
                });
            if let Some(r) = &current {
                ui.label(
                    RichText::new(r.to_string())
                        .monospace()
                        .color(LIGHT.secondary),
                );
            }
            if choice != current {
                edit = Some(Command::SetBlock { block: choice });
            }
        });

        if let Some(assembly) = &circuit.assembly
            && let Some(block) = catalogue.block(&assembly.block)
        {
            theme::section(ui, |ui| {
                theme::column_title(ui, "FACES");
                let width = ui.available_width();
                // The table's edge, its cells' padding and the gap between them.
                let face_width = 84.0;
                let valve_width = (width - 2.0 - 24.0 - 12.0 - face_width).max(60.0);
                theme::table(ui, width, |ui| {
                    theme::table_row(ui, LIGHT.panel, |ui| {
                        theme::cell(ui, face_width, |ui| {
                            theme::column_title(ui, "FACE");
                        });
                        theme::cell(ui, valve_width, |ui| {
                            theme::column_title(ui, "VALVE");
                        });
                    });
                    for face in &block.mount_faces {
                        let holder = assembly
                            .mounts
                            .iter()
                            .find(|(_, f)| **f == face.name)
                            .map(|(c, _)| c.clone());
                        let mut choice = holder.clone();
                        theme::table_row(ui, Color32::TRANSPARENT, |ui| {
                            theme::cell(ui, face_width, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.monospace(&face.name).on_hover_text(&face.pattern);
                                ui.label(
                                    RichText::new(&face.pattern).small().color(LIGHT.secondary),
                                );
                            });
                            theme::cell(ui, valve_width, |ui| {
                                egui::ComboBox::from_id_salt(("face", &face.name))
                                    .selected_text(choice.as_deref().unwrap_or("—"))
                                    .width(valve_width)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut choice, None, "—");
                                        // Only valves whose mounting pattern fits this face.
                                        for c in &circuit.components {
                                            // Red valves and items have no mounting to fit.
                                            let Some(valve_type) =
                                                c.valve().and_then(|v| v.valve_type.as_ref())
                                            else {
                                                continue;
                                            };
                                            let fits =
                                                catalogue.valve_type(valve_type).is_some_and(|v| {
                                                    v.mounting.as_deref()
                                                        == Some(face.pattern.as_str())
                                                });
                                            if fits {
                                                ui.selectable_value(
                                                    &mut choice,
                                                    Some(c.id.clone()),
                                                    &c.id,
                                                );
                                            }
                                        }
                                    });
                            });
                        });
                        if choice != holder {
                            edit = Some(match (choice, holder) {
                                (Some(c), _) => Command::Mount {
                                    component: c,
                                    face: Some(face.name.clone()),
                                },
                                (None, Some(h)) => Command::Mount {
                                    component: h,
                                    face: None,
                                },
                                (None, None) => unreachable!("choice differs from holder"),
                            });
                        }
                    }
                });
            });
        }

        // The valve selected in the schematic: what it is, its catalogue ref and its face.
        if let Some(c) = editor.selected().and_then(|id| circuit.component(id))
            && let ComponentKind::Valve(v) = &c.kind
        {
            theme::section(ui, |ui| {
                theme::column_title(ui, "SELECTED");
                ui.horizontal_wrapped(|ui| {
                    let id = RichText::new(&c.id)
                        .family(egui::FontFamily::Name(theme::MONO_SEMIBOLD.into()));
                    ui.label(id);
                    if let Some(t) = v.valve_type.as_ref().and_then(|r| catalogue.valve_type(r)) {
                        let what = format!("{} {}", t.series, t.code);
                        ui.label(RichText::new(what).color(LIGHT.secondary));
                    }
                });
                if let Some(r) = &v.valve_type {
                    ui.label(
                        RichText::new(r.to_string())
                            .monospace()
                            .color(LIGHT.secondary),
                    );
                }
                let face = (circuit.assembly.as_ref()).and_then(|a| a.mounts.get(&c.id));
                if let Some(face) = face {
                    ui.label(format!("On face {face}"));
                }
            });
        }

        if let Some(command) = edit {
            self.status = editor.edit(command).err();
        }
        let circuit = editor.circuit();
        let issues = circuit
            .validate(catalogue)
            .into_iter()
            .filter(is_assembly_issue);
        let lines: Vec<Note> = issues
            .map(|issue| {
                let mark = match issue.severity() {
                    Severity::Error => Mark::Error,
                    Severity::Warning => Mark::Warning,
                };
                (mark, issue.to_string())
            })
            .chain(notes.iter().cloned())
            .chain(self.status.clone().map(|s| (Mark::Error, s)))
            .collect();
        let offline = shared.db.is_none();
        if !lines.is_empty() || offline {
            theme::section(ui, |ui| {
                for (mark, line) in lines {
                    theme::marked(ui, mark, line);
                }
                if offline {
                    let hint = "3D models come from the shared database.";
                    ui.label(RichText::new(hint).color(LIGHT.secondary));
                }
            });
        }
    }

    fn pick_preview(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("glTF model", &["glb"])
            .pick_file()
        else {
            return;
        };
        let name = path
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into());
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Preview::load(name, &bytes))
        {
            Ok(p) => self.preview = Some(p),
            Err(e) => self.status = Some(format!("Could not preview {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use hydra_core::CatalogueRef;

    use super::*;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    fn shared() -> Shared {
        let text = std::fs::read_to_string(format!("{ROOT}/fixtures/catalogue.json")).unwrap();
        Shared::new(serde_json::from_str(&text).unwrap())
    }

    fn demo() -> Circuit {
        let path = format!("{ROOT}/fixtures/circuits/demo-manifold.json");
        Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn with_models(mut shared: Shared) -> Shared {
        shared
            .block_models
            .insert(CatalogueRef::new("blk-demo", 1), "block".into());
        shared.valve_models.insert(
            CatalogueRef::new("vt-demo-4-3-closed", 1),
            "spool-model".into(),
        );
        shared
    }

    #[test]
    fn scene_mounts_valves_on_their_faces_and_names_missing_models() {
        let shared = with_models(shared());
        let (placed, notes) = scene(&demo(), None, &shared);
        let block = shared
            .catalogue
            .block(&CatalogueRef::new("blk-demo", 1))
            .unwrap();
        assert_eq!(
            placed,
            vec![
                Placed {
                    sha256: "block".into(),
                    transform: IDENTITY,
                    component: None,
                },
                Placed {
                    sha256: "spool-model".into(),
                    transform: block.face("F1").unwrap().transform().unwrap(),
                    component: Some("V1".into()),
                },
            ]
        );
        assert_eq!(
            notes,
            [(Mark::Warning, "V2 (DEMO-06 4-2-S) has no 3D model.".into())]
        );

        // Without a block, the selected valve is shown on its own.
        let mut loose = demo();
        loose.assembly = None;
        assert_eq!(scene(&loose, None, &shared).0, []);
        let (placed, _) = scene(&loose, Some("V1"), &shared);
        assert_eq!(placed[0].transform, IDENTITY);
        assert_eq!(placed[0].component.as_deref(), Some("V1"));
        assert_eq!(scene(&loose, Some("P"), &shared).0, []);

        let (placed, notes) = scene(&demo(), None, &self::shared());
        assert_eq!(placed, []);
        assert_eq!(
            notes[0],
            (
                Mark::Warning,
                "Block Demo manifold block (fixture data, matches fixtures/models/demo-block.step) has no 3D model.".into()
            )
        );
    }

    type State = (ModelPanel, Shared, SchematicEditor);

    fn harness(circuit: Circuit) -> Harness<'static, State> {
        let mut h = theme::harness(
            egui::Vec2::new(900.0, 800.0),
            |ui, (p, s, e): &mut State| p.ui(ui, s, e),
            (
                // Paints nothing without the GPU resources the app installs.
                ModelPanel::new(Some(Viewer3d::unfitted())),
                shared(),
                SchematicEditor::new(circuit, None),
            ),
        );
        h.run();
        h
    }

    fn mounts(h: &Harness<State>) -> Vec<(String, String)> {
        let circuit = h.state().2.circuit();
        let assembly = circuit.assembly.as_ref().expect("a block");
        assembly.mounts.clone().into_iter().collect()
    }

    /// Open combo box `n` (0 is the block, then one per face) and pick `option`.
    fn choose(h: &mut Harness<State>, n: usize, option: &str) {
        h.get_all_by_role(egui::accesskit::Role::ComboBox)
            .nth(n)
            .expect("combo box")
            .click();
        h.run();
        h.get_by_label(option).click();
        h.run();
    }

    #[test]
    fn choosing_a_block_and_mounting_valves_on_faces() {
        let mut loose = demo();
        loose.assembly = None;
        let mut h = harness(loose);
        let hint =
            "Choose a block for the circuit, or select a valve in the schematic, to see it in 3D.";
        assert!(h.query_by_label(hint).is_some());
        choose(&mut h, 0, BLOCK_V2);
        // The panel says why nothing shows: no models, nothing mounted.
        assert!(h.query_by_label(hint).is_none());
        assert_eq!(mounts(&h), []);
        assert!(h.query_by_label("V1 is not mounted on the block").is_some());

        // F1 takes CETOP 3 valves only: V1 and V2, not the cartridge valves.
        h.get_all_by_role(egui::accesskit::Role::ComboBox)
            .nth(1)
            .unwrap()
            .click();
        h.run();
        assert!(h.query_by_label("RV1").is_none());
        assert!(h.query_by_label("V1").is_some());
        h.get_by_label("V2").click();
        h.run();
        assert_eq!(mounts(&h), [("V2".into(), "F1".into())]);
        // F3 is CETOP 5: none of the circuit's valves fit.
        h.get_all_by_role(egui::accesskit::Role::ComboBox)
            .nth(3)
            .unwrap()
            .click();
        h.run();
        assert!(h.query_by_label("V1").is_none());
        h.key_press(egui::Key::Escape);
        h.run();

        // Moving V2 to F2 frees F1; emptying F2 unmounts it.
        choose(&mut h, 2, "V2");
        assert_eq!(mounts(&h), [("V2".into(), "F2".into())]);
        // Picking V1 for F2 replaces V2 there.
        choose(&mut h, 2, "V1");
        assert_eq!(mounts(&h), [("V1".into(), "F2".into())]);
        choose(&mut h, 2, "—");
        assert_eq!(mounts(&h), []);
        choose(&mut h, 0, "None");
        assert_eq!(h.state().2.circuit().assembly, None);
    }

    #[test]
    fn only_valves_in_the_block_are_expected_on_it() {
        let mut c = demo();
        c.assembly = None;
        c.in_block.remove("V2");
        let mut h = harness(c);
        choose(&mut h, 0, BLOCK_V2);
        assert!(h.query_by_label("V1 is not mounted on the block").is_some());
        assert!(h.query_by_label("V2 is not mounted on the block").is_none());

        let outside = "V2 is mounted on the block but drawn outside it; tick In block in the schematic, or take it off its face in the 3D tab";
        // Mounting V2 puts it in the block too.
        choose(&mut h, 1, "V2");
        assert!(h.state().2.circuit().in_block.contains("V2"));
        assert!(h.query_by_label(outside).is_none());
        // Unticking "In block" in the schematic keeps the mount and warns.
        h.state_mut()
            .2
            .edit(Command::SetInBlock {
                id: "V2".into(),
                inside: false,
            })
            .unwrap();
        h.run();
        assert!(h.query_by_label(outside).is_some());
        assert_eq!(mounts(&h), [("V2".into(), "F1".into())]);

        // With nothing in the block, the tab says why it can't check the mounts.
        let mut c = demo();
        c.in_block.clear();
        let h = harness(c);
        assert!(
            h.query_by_label_contains("No part is ticked In block")
                .is_some()
        );
        assert!(
            h.query_by_label_contains("mounted on the block but")
                .is_none()
        );
    }

    const BLOCK_V2: &str = "Demo manifold block with cartridge cavity C1 (fixture data, matches fixtures/models/demo-block.step) (v2)";

    #[test]
    fn faces_never_offer_missing_valves_or_items() {
        let path = format!("{ROOT}/fixtures/circuits/demo-import-manifold.json");
        let c = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut h = harness(c);
        choose(&mut h, 0, BLOCK_V2);
        // F3 is CETOP 5: 5.2 asks for one but is missing from the catalogue.
        h.get_all_by_role(egui::accesskit::Role::ComboBox)
            .nth(3)
            .unwrap()
            .click();
        h.run();
        assert!(h.query_by_label("5.2").is_none());
        h.key_press(egui::Key::Escape);
        h.run();
        // C1 takes the relief cartridge, not the missing 2/2 cartridge or the orifices.
        h.get_all_by_role(egui::accesskit::Role::ComboBox)
            .nth(4)
            .unwrap()
            .click();
        h.run();
        assert!(h.query_by_label("7.1").is_some());
        for absent in ["4.1", "X1", "X2", "M1"] {
            assert!(h.query_by_label(absent).is_none(), "{absent}");
        }
        h.get_by_label("7.1").click();
        h.run();
        assert_eq!(mounts(&h), [("7.1".into(), "C1".into())]);
    }

    #[test]
    fn a_preview_shows_the_file_alone_with_its_size() {
        let bytes = std::fs::read(format!("{ROOT}/fixtures/models/demo-valve-cetop3.glb")).unwrap();
        let mut h = harness(demo());
        h.state_mut().0.preview = Some(Preview::load("valve.glb".into(), &bytes).unwrap());
        h.run();
        assert!(
            h.query_by_label("139.5 x 44.0 x 52.0 mm, z 0.0..52.0")
                .is_some()
        );
        // The circuit's block is out of the way.
        assert!(h.query_by_label("BLOCK").is_none());
        h.get_by_label("Close preview").click();
        h.run();
        assert!(h.state().0.preview.is_none());
        assert!(h.query_by_label("BLOCK").is_some());
        assert!(Preview::load("x".into(), b"not a model").is_err());
    }

    /// The colour `text` is painted in, if it is.
    fn colour_of(h: &Harness<State>, text: &str) -> Option<Color32> {
        h.output().shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == text => {
                Some(t.galley.job.sections[0].format.color)
            }
            _ => None,
        })
    }

    #[test]
    fn the_panel_marks_problems_in_their_tone_and_hints_in_secondary() {
        let mut c = demo();
        // F3 is CETOP 5: V1 doesn't fit it.
        c.assembly
            .as_mut()
            .unwrap()
            .mounts
            .insert("V1".into(), "F3".into());
        let mismatch = c
            .validate(&shared().catalogue)
            .into_iter()
            .find(|i| matches!(i, Issue::PatternMismatch { .. }))
            .expect("V1 doesn't fit F3")
            .to_string();
        let h = harness(c);
        for title in ["BLOCK", "FACES", "FACE", "VALVE"] {
            assert!(h.query_by_label(title).is_some(), "{title}");
        }
        // The block's ref in Data type.
        assert!(h.query_by_label("blk-demo@1").is_some());
        assert_eq!(colour_of(&h, &mismatch), Some(LIGHT.missing.text));
        let no_model = "Block Demo manifold block (fixture data, matches fixtures/models/demo-block.step) has no 3D model.";
        assert_eq!(colour_of(&h, no_model), Some(LIGHT.unlisted.text));
        let offline = "3D models come from the shared database.";
        assert_eq!(colour_of(&h, offline), Some(LIGHT.secondary));
        // Nothing selected, nothing about a selection.
        assert!(h.query_by_label("SELECTED").is_none());
        assert!(h.query_by_label("3D needs the wgpu renderer.").is_none());
        // Without the renderer, that is what the view says, whatever else is going on.
        let mut loose = demo();
        loose.assembly = None;
        let mut h = harness(loose);
        h.state_mut().0.viewer = None;
        h.run();
        assert!(h.query_by_label("3D needs the wgpu renderer.").is_some());
        assert!(h.query_by_label_contains("Choose a block").is_none());
    }

    #[test]
    fn a_model_that_fails_to_load_is_an_error_and_an_unknown_block_needs_no_hint() {
        let mut h = harness(demo());
        h.state_mut().1 = with_models(shared());
        h.state_mut()
            .0
            .loaded("block".into(), Err("not a glTF file".into()));
        h.run();
        assert_eq!(
            colour_of(&h, "The block: not a glTF file"),
            Some(LIGHT.missing.text)
        );

        // A block the catalogue doesn't have: its error says so, not the "choose" hint.
        let mut c = demo();
        c.assembly.as_mut().unwrap().block = CatalogueRef::new("blk-gone", 1);
        let unknown = c
            .validate(&shared().catalogue)
            .into_iter()
            .find(|i| matches!(i, Issue::UnknownBlock(_)))
            .expect("an unknown block")
            .to_string();
        let h = harness(c);
        assert_eq!(colour_of(&h, &unknown), Some(LIGHT.missing.text));
        assert!(h.query_by_label_contains("Choose a block").is_none());
    }

    #[test]
    fn the_selected_valve_shows_its_ref_and_face() {
        let mut h = harness(demo());
        h.state_mut().2.select("V1".into());
        h.run();
        assert!(h.query_by_label("SELECTED").is_some());
        assert!(h.query_by_label("vt-demo-4-3-closed@1").is_some());
        assert!(h.query_by_label("On face F1").is_some());
        // A port is no valve: nothing to show.
        h.state_mut().2.select("P".into());
        h.run();
        assert!(h.query_by_label("SELECTED").is_none());
    }

    #[test]
    fn models_are_grey_and_the_selected_valve_takes_the_accent() {
        assert_eq!(tint(None, Some("V1")), LIGHT.border);
        assert_eq!(tint(Some("V2"), Some("V1")), LIGHT.border_strong);
        assert_eq!(tint(Some("V1"), Some("V1")), LIGHT.accent);
    }

    #[test]
    fn a_missing_valve_has_no_model() {
        let path = format!("{ROOT}/fixtures/circuits/demo-import-manifold.json");
        let c = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (placed, notes) = scene(&c, Some("5.2"), &shared());
        assert_eq!(placed, []);
        assert_eq!(
            notes,
            [(Mark::Error, "5.2 is missing from the catalogue.".into())]
        );
        assert_eq!(scene(&c, Some("X1"), &shared()).0, []);
    }
}
