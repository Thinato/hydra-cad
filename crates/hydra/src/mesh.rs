//! Triangle meshes flattened from glTF assets.

use anyhow::{Context, bail};
use glam::{Mat3, Mat4, Vec3};

/// A single indexed triangle list in world space.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// Loads a `.glb` (or self-contained `.gltf`) and flattens every triangle
/// primitive of the default scene into one mesh, with node transforms applied.
pub fn load_glb(bytes: &[u8]) -> anyhow::Result<Mesh> {
    let (doc, buffers, _images) = gltf::import_slice(bytes).context("parsing glTF")?;
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .context("glTF has no scene")?;
    let mut mesh = Mesh::default();
    let mut stack: Vec<_> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
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
            let Some(pos) = reader.read_positions() else {
                bail!("primitive without POSITION");
            };
            let pos: Vec<Vec3> = pos.map(|p| world.transform_point3(p.into())).collect();
            let mut idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            if idx.iter().any(|&i| i as usize >= pos.len()) {
                bail!("primitive index out of range");
            }
            if flip {
                idx.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
            }
            let base = mesh.positions.len() as u32;
            match reader.read_normals() {
                Some(nrm) => {
                    mesh.positions.extend(pos.iter().map(|p| p.to_array()));
                    mesh.normals.extend(
                        nrm.map(|n| (normal_mat * Vec3::from(n)).normalize_or_zero().to_array()),
                    );
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
    if mesh.indices.is_empty() {
        bail!("glTF has no triangles to draw");
    }
    Ok(mesh)
}

impl Mesh {
    /// Axis-aligned bounds `(min, max)`.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let (lo, hi) = self.positions.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &p| (lo.min(p.into()), hi.max(p.into())),
        );
        (lo.to_array(), hi.to_array())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3D smoke test: every asset must load headless with sane geometry.
    #[test]
    fn all_assets_load() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).expect("assets dir") {
            let path = entry.unwrap().path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "glb" | "gltf") {
                continue;
            }
            count += 1;
            let mesh = load_glb(&std::fs::read(&path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let name = path.display();
            let n = mesh.positions.len();
            assert!(n > 0, "{name}: no vertices");
            assert_eq!(mesh.normals.len(), n, "{name}: normal count");
            assert_eq!(mesh.indices.len() % 3, 0, "{name}: indices not triangles");
            assert!(
                mesh.indices.iter().all(|&i| (i as usize) < n),
                "{name}: index out of range"
            );
            let (lo, hi) = mesh.bounds();
            for k in 0..3 {
                assert!(lo[k].is_finite() && hi[k].is_finite(), "{name}: bounds");
                assert!(hi[k] - lo[k] > 0.0, "{name}: degenerate extent on axis {k}");
            }
        }
        assert!(count > 0, "no glTF assets in {dir}");
    }
}
