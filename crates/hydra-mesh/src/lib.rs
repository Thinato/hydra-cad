//! Triangle meshes flattened from glTF assets, in Hydra's model space.
//!
//! glTF files are Y-up and in metres (the glTF standard). Hydra's model space is the CAD one:
//! millimetres, Z up, so mount faces and models share coordinates.

use glam::{Mat3, Mat4, Vec3};

/// glTF (Y-up, metres) to Hydra model space (Z-up, millimetres): (x, y, z) -> (x, -z, y) * 1000.
const FROM_GLTF: Mat4 = Mat4::from_cols_array(&[
    1000.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1000.0, 0.0, //
    0.0, -1000.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
]);

/// Most triangles a model may have. Keeps vertex buffers well inside wgpu's default 256 MB
/// buffer limit; a finer mesh than this is a conversion tolerance problem.
pub const MAX_TRIANGLES: usize = 4_000_000;

/// A single indexed triangle list in model space.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// Loads a `.glb` (or self-contained `.gltf`) and flattens every triangle primitive of the
/// default scene into one mesh, with node transforms applied, then [`Mesh::check`]s it.
///
/// Files come from the database and disk, so a malformed one is an `Err`, never a hang or a
/// panic.
pub fn load_glb(bytes: &[u8]) -> Result<Mesh, String> {
    // The gltf crate can panic on some malformed accessors (arithmetic overflow).
    std::panic::catch_unwind(|| load(bytes))
        .unwrap_or_else(|_| Err("not a readable glTF file: malformed accessor".into()))
}

fn load(bytes: &[u8]) -> Result<Mesh, String> {
    let unreadable = |e: gltf::Error| format!("not a readable glTF file: {e}");
    // Images are never drawn, so they are not decoded.
    let gltf = gltf::Gltf::from_slice(bytes).map_err(unreadable)?;
    let buffers =
        gltf::import_buffers(&gltf.document, None, gltf.blob.clone()).map_err(unreadable)?;
    let doc = &gltf.document;
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or("the glTF file has no scene")?;
    let mut mesh = Mesh::default();
    let mut stack: Vec<_> = scene.nodes().map(|n| (n, FROM_GLTF)).collect();
    // The node graph must be a tree; a cycle would never end and shared nodes multiply.
    let mut visited = vec![false; doc.nodes().len()];
    while let Some((node, parent)) = stack.pop() {
        if std::mem::replace(&mut visited[node.index()], true) {
            return Err("the glTF node tree has a loop or a shared node".into());
        }
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|c| (c, world)));
        let Some(m) = node.mesh() else { continue };
        let normal_mat = Mat3::from_mat4(world).inverse().transpose();
        let flip = world.determinant() < 0.0;
        for prim in m.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                tracing::warn!(
                    "skipping {:?} primitive, only triangle lists are drawn",
                    prim.mode()
                );
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let pos: Vec<Vec3> = reader
                .read_positions()
                .ok_or("a primitive has no POSITION")?
                .map(|p| world.transform_point3(p.into()))
                .collect();
            let mut idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            if idx.iter().any(|&i| i as usize >= pos.len()) {
                return Err("a primitive index is out of range".into());
            }
            idx.truncate(idx.len() / 3 * 3);
            if flip {
                idx.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
            }
            if mesh.indices.len() + idx.len() > 3 * MAX_TRIANGLES {
                return Err(too_detailed());
            }
            let base = mesh.positions.len() as u32;
            match reader.read_normals() {
                Some(nrm) => {
                    let nrm: Vec<_> = nrm
                        .map(|n| (normal_mat * Vec3::from(n)).normalize_or_zero().to_array())
                        .collect();
                    if nrm.len() != pos.len() {
                        return Err(
                            "a primitive has a different number of normals and positions".into(),
                        );
                    }
                    mesh.positions.extend(pos.iter().map(|p| p.to_array()));
                    mesh.normals.extend(nrm);
                    mesh.indices.extend(idx.iter().map(|i| base + i));
                }
                None => {
                    // Flat shading: one vertex per triangle corner.
                    for t in idx.chunks_exact(3) {
                        let [a, b, c] = [0, 1, 2].map(|k| pos[t[k] as usize]);
                        let n = (b - a).cross(c - a).normalize_or_zero().to_array();
                        let i = mesh.positions.len() as u32;
                        mesh.positions.extend([a, b, c].map(|p| p.to_array()));
                        mesh.normals.extend([n; 3]);
                        mesh.indices.extend([i, i + 1, i + 2]);
                    }
                }
            }
        }
    }
    mesh.check()?;
    Ok(mesh)
}

fn too_detailed() -> String {
    format!(
        "the model has more than {MAX_TRIANGLES} triangles; convert it with a coarser tolerance"
    )
}

impl Mesh {
    /// Axis-aligned bounds `(min, max)` in millimetres.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let (lo, hi) = self.positions.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &p| (lo.min(p.into()), hi.max(p.into())),
        );
        (lo.to_array(), hi.to_array())
    }

    /// The 3D smoke test: triangles to draw, consistent buffers, finite and non-flat bounds.
    pub fn check(&self) -> Result<(), String> {
        let n = self.positions.len();
        if self.indices.is_empty() {
            return Err("the model has no triangles to draw".into());
        }
        if self.indices.len() > 3 * MAX_TRIANGLES || n > 3 * MAX_TRIANGLES {
            return Err(too_detailed());
        }
        if self.normals.len() != n || !self.indices.len().is_multiple_of(3) {
            return Err("the model's vertex buffers are inconsistent".into());
        }
        if self.indices.iter().any(|&i| i as usize >= n) {
            return Err("a triangle refers to a vertex that does not exist".into());
        }
        // Checked per value: min/max over a NaN can hide it.
        if !self
            .positions
            .iter()
            .chain(&self.normals)
            .flatten()
            .all(|c| c.is_finite())
        {
            return Err("the model has coordinates that are not numbers".into());
        }
        let (lo, hi) = self.bounds();
        if (0..3).any(|k| hi[k] - lo[k] <= 0.0) {
            return Err("the model is flat or a single point".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<(String, Vec<u8>)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/models");
        let mut found: Vec<_> = std::fs::read_dir(dir)
            .expect("fixtures/models")
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "glb"))
            .map(|p| (p.display().to_string(), std::fs::read(&p).unwrap()))
            .collect();
        found.sort();
        assert!(!found.is_empty(), "no glb files in {dir}");
        found
    }

    /// Every fixture model loads headless with sane geometry.
    #[test]
    fn fixture_models_load() {
        for (name, bytes) in models() {
            let mesh = load_glb(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(mesh.indices.len() >= 12, "{name}: too few triangles");
        }
    }

    /// The converter's demo valve sits on z = 0 in millimetres: the glTF -> model space
    /// conversion is right.
    #[test]
    fn demo_valve_is_in_millimetres_z_up() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/models/demo-valve-cetop3.glb"
        );
        let mesh = load_glb(&std::fs::read(path).unwrap()).unwrap();
        let (lo, hi) = mesh.bounds();
        let near = |a: f32, b: f32| (a - b).abs() < 0.5;
        assert!(near(lo[2], 0.0) && near(hi[2], 52.0), "z {lo:?}..{hi:?}");
        assert!(near(lo[1], -6.0) && near(hi[1], 38.0), "y {lo:?}..{hi:?}");
        assert!(near(lo[0], -49.5) && near(hi[0], 90.0), "x {lo:?}..{hi:?}");
    }

    /// Malformed files from the database must fail, not hang the worker or panic it.
    #[test]
    fn hostile_files_fail_cleanly() {
        let cycle = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "children": [1]}, {"children": [0]}], "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}], "buffers": [{"byteLength": 72, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAIA/AADAfwAAwH8AAMB/AAAAAAAAAAAAAIA/AACAPwAAgD8AAIA/"}], "bufferViews": [{"buffer": 0, "byteLength": 72}], "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 1]}]}"#;
        assert_eq!(
            load_glb(cycle.as_bytes()).unwrap_err(),
            "the glTF node tree has a loop or a shared node"
        );
        let empty_accessor = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0}], "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}], "buffers": [{"byteLength": 36, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAIA/"}], "bufferViews": [{"buffer": 0, "byteLength": 36}], "accessors": [{"bufferView": 0, "componentType": 5126, "count": 0, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 1]}]}"#;
        assert!(
            load_glb(empty_accessor.as_bytes())
                .unwrap_err()
                .starts_with("not a readable glTF file")
        );
        // Six vertices, the fourth NaN: hidden from min/max, caught per value.
        let nan = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0}], "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}], "buffers": [{"byteLength": 72, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAIA/AADAfwAAwH8AAMB/AAAAAAAAAAAAAIA/AACAPwAAgD8AAIA/"}], "bufferViews": [{"buffer": 0, "byteLength": 72}], "accessors": [{"bufferView": 0, "componentType": 5126, "count": 6, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 1]}]}"#;
        assert_eq!(
            load_glb(nan.as_bytes()).unwrap_err(),
            "the model has coordinates that are not numbers"
        );
    }

    #[test]
    fn broken_files_are_refused_in_plain_words() {
        assert!(
            load_glb(b"not a model")
                .unwrap_err()
                .starts_with("not a readable glTF file")
        );
        let flat = Mesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
        };
        assert_eq!(
            flat.check(),
            Err("the model is flat or a single point".into())
        );
        let dangling = Mesh {
            indices: vec![0, 1, 5],
            ..flat.clone()
        };
        assert_eq!(
            dangling.check(),
            Err("a triangle refers to a vertex that does not exist".into())
        );
        assert_eq!(
            Mesh::default().check(),
            Err("the model has no triangles to draw".into())
        );
    }
}
