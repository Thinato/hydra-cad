//! 3D tab: the circuit's manifold block with its valves mounted on their faces, or else the
//! valve selected in the schematic. Models download once and are cached by hash.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use glam::Mat4;
use hydra_core::{Circuit, Command, ComponentKind, Issue, Severity};
use hydra_mesh::Mesh;

use crate::catalogue::Shared;
use crate::db::Request;
use crate::editor::SchematicEditor;
use crate::viewer3d::{Instance, Viewer3d};

const BLOCK_COLOUR: [f32; 3] = [0.62, 0.64, 0.68];
const VALVE_COLOUR: [f32; 3] = [0.25, 0.45, 0.75];
const SELECTED_COLOUR: [f32; 3] = [0.95, 0.6, 0.15];

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

/// What the 3D view shows for `circuit`, and notes on what it can't show.
pub fn scene(
    circuit: &Circuit,
    selected: Option<&str>,
    shared: &Shared,
) -> (Vec<Placed>, Vec<String>) {
    let catalogue = &shared.catalogue;
    let (mut placed, mut notes) = (Vec::new(), Vec::new());
    let valve_model = |component: &str, notes: &mut Vec<String>| {
        let ComponentKind::Valve { valve_type } = &circuit.component(component)?.kind else {
            return None;
        };
        let model = shared.valve_models.get(valve_type).cloned();
        if model.is_none() {
            let name = catalogue
                .valve_type(valve_type)
                .map_or(valve_type.to_string(), |v| {
                    format!("{} {}", v.series, v.code)
                });
            notes.push(format!("{component} ({name}) has no 3D model."));
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
                None => notes.push(format!("Block {} has no 3D model.", block.name)),
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
                    Err(e) => notes.push(format!("Block {}: {e}", block.name)),
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
    )
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
}

impl ModelPanel {
    pub fn new(viewer: Option<Viewer3d>) -> Self {
        Self {
            viewer,
            models: HashMap::new(),
            status: None,
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
        let (placed, mut notes) = scene(editor.circuit(), editor.selected(), shared);
        if let Some(db) = &shared.db {
            for p in &placed {
                if !self.models.contains_key(&p.sha256) {
                    db.send(Request::LoadModel(p.sha256.clone()));
                    self.models.insert(p.sha256.clone(), Model::Loading);
                }
            }
        } else {
            notes.push("3D models come from the shared database.".into());
        }
        for p in &placed {
            if let Some(Model::Failed(e)) = self.models.get(&p.sha256) {
                let what = p.component.as_deref().unwrap_or("The block");
                notes.push(format!("{what}: {e}"));
            }
        }

        egui::Panel::left("assembly")
            .default_size(210.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
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
                    color: match &p.component {
                        None => BLOCK_COLOUR,
                        Some(c) if Some(c.as_str()) == editor.selected() => SELECTED_COLOUR,
                        Some(_) => VALVE_COLOUR,
                    },
                }),
                Some(Model::Failed(_)) => {}
                _ => loading += 1,
            }
        }
        if loading > 0 {
            ui.weak(format!("Loading {loading} model(s)…"));
        } else if placed.is_empty() {
            ui.weak("Choose a block for the circuit, or select a valve in the schematic, to see it in 3D.");
        }
        match &mut self.viewer {
            Some(viewer) => viewer.ui(ui, &scene),
            None => {
                ui.label("3D needs the wgpu renderer.");
            }
        }
    }

    fn assembly(
        &mut self,
        ui: &mut egui::Ui,
        shared: &Shared,
        editor: &mut SchematicEditor,
        notes: &[String],
    ) {
        let catalogue = &shared.catalogue;
        let circuit = editor.circuit();
        let current = circuit.assembly.as_ref().map(|a| a.block.clone());
        let mut edit = None;

        ui.heading("Block");
        let label = |r: &Option<hydra_core::CatalogueRef>| match r {
            None => "None".to_string(),
            Some(r) => catalogue
                .block(r)
                .map_or(r.to_string(), |b| format!("{} (v{})", b.name, b.version)),
        };
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
        if choice != current {
            edit = Some(Command::SetBlock { block: choice });
        }

        if let Some(assembly) = &circuit.assembly
            && let Some(block) = catalogue.block(&assembly.block)
        {
            ui.add_space(6.0);
            ui.heading("Faces");
            egui::Grid::new("faces").num_columns(2).show(ui, |ui| {
                for face in &block.mount_faces {
                    ui.label(&face.name).on_hover_text(&face.pattern);
                    let holder = assembly
                        .mounts
                        .iter()
                        .find(|(_, f)| **f == face.name)
                        .map(|(c, _)| c.clone());
                    let mut choice = holder.clone();
                    egui::ComboBox::from_id_salt(("face", &face.name))
                        .selected_text(choice.as_deref().unwrap_or("—"))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut choice, None, "—");
                            // Only valves whose mounting pattern fits this face.
                            for c in &circuit.components {
                                let ComponentKind::Valve { valve_type } = &c.kind else {
                                    continue;
                                };
                                let fits = catalogue.valve_type(valve_type).is_some_and(|v| {
                                    v.mounting.as_deref() == Some(face.pattern.as_str())
                                });
                                if fits {
                                    ui.selectable_value(&mut choice, Some(c.id.clone()), &c.id);
                                }
                            }
                        });
                    ui.end_row();
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
                    ui.weak(&face.pattern);
                    ui.end_row();
                }
            });
        }

        ui.add_space(6.0);
        for issue in circuit
            .validate(catalogue)
            .into_iter()
            .filter(is_assembly_issue)
        {
            let colour = match issue.severity() {
                Severity::Error => ui.visuals().error_fg_color,
                Severity::Warning => ui.visuals().warn_fg_color,
            };
            ui.colored_label(colour, issue.to_string());
        }
        for note in notes {
            ui.weak(note);
        }
        if let Some(command) = edit {
            self.status = editor.edit(command).err();
        }
        if let Some(s) = &self.status {
            ui.colored_label(ui.visuals().warn_fg_color, s);
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
        shared
            .valve_models
            .insert(CatalogueRef::new("vt-rpe3-06-z11", 1), "rpe3".into());
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
                    sha256: "rpe3".into(),
                    transform: block.face("F1").unwrap().transform().unwrap(),
                    component: Some("V1".into()),
                },
            ]
        );
        assert_eq!(notes, ["V2 (DEMO-06 4-2-S) has no 3D model."]);

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
            "Block Demo manifold block (fixture data, matches fixtures/models/demo-block.step) has no 3D model."
        );
    }

    type State = (ModelPanel, Shared, SchematicEditor);

    fn harness(circuit: Circuit) -> Harness<'static, State> {
        let mut h = Harness::builder()
            .with_size(egui::Vec2::new(900.0, 600.0))
            .build_ui_state(
                |ui, (p, s, e): &mut State| p.ui(ui, s, e),
                (
                    ModelPanel::new(None),
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
        choose(
            &mut h,
            0,
            "Demo manifold block (fixture data, matches fixtures/models/demo-block.step) (v1)",
        );
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
}
