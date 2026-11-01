//! Triangle meshes flattened from glTF assets, in Hydra's model space.
//!
//! glTF files are Y-up and in metres (the glTF standard). Hydra's model space is the CAD one:
//! millimetres, Z up, so mount faces and models share coordinates.

use glam::{DMat3, DMat4, DQuat, DVec3, Mat3, Mat4, Vec3};

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

/// A guess at how to stand a screw-in cartridge up, as a column-major model-space matrix: its
/// long axis onto +z with the wider end up, the axis through x = y = 0, and z = 0 at the
/// largest step out in radius (the seating shoulder), else at the bottom.
pub fn guess_alignment(mesh: &Mesh) -> [[f64; 4]; 4] {
    let pts: Vec<DVec3> = mesh
        .positions
        .iter()
        .map(|&p| Vec3::from(p).as_dvec3())
        .collect();
    if pts.is_empty() {
        return DMat4::IDENTITY.to_cols_array_2d();
    }
    let mean = pts.iter().sum::<DVec3>() / pts.len() as f64;
    let mut cov = DMat3::ZERO;
    for p in &pts {
        let d = *p - mean;
        cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    // Power iteration, from the column with the most of the dominant direction in it.
    let mut axis = [cov.x_axis, cov.y_axis, cov.z_axis]
        .into_iter()
        .max_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap_or(DVec3::Z)
        .normalize_or(DVec3::Z);
    for _ in 0..100 {
        axis = (cov * axis).normalize_or(axis);
    }
    let mut rot = DQuat::from_rotation_arc(axis, DVec3::Z);
    let mut profile = Profile::new(&pts, &mesh.indices, rot);
    // Wider end up: more radius along the upper half than the lower.
    let (lower, upper) = profile.radius.split_at(Profile::SLICES / 2);
    if lower.iter().sum::<f64>() > upper.iter().sum::<f64>() {
        rot = DQuat::from_rotation_x(std::f64::consts::PI) * rot;
        profile = Profile::new(&pts, &mesh.indices, rot);
    }
    // ponytail: the largest step out wins, so a solenoid coil wider than its hex gets the
    // coil's shoulder; the admin's --offset corrects it.
    let widest = profile.radius.iter().copied().fold(0.0, f64::max);
    let step = profile
        .radius
        .windows(2)
        .enumerate()
        .map(|(i, w)| (i, w[1] - w[0]))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let z0 = match step {
        Some((i, d)) if d > 0.05 * widest => {
            // Snap to the lowest corner of the wider part at the step: rounding can put the
            // flat face into the slice below.
            let edge = profile.z0 + (i + 1) as f64 * profile.slice;
            let wide = profile.radius[i] + d / 2.0;
            (profile.pts.iter())
                .filter(|p| (p.z - edge).abs() <= 2.0 * profile.slice && profile.r(p) > wide)
                .map(|p| p.z)
                .reduce(f64::min)
                .unwrap_or(edge)
        }
        _ => profile.z0,
    };
    let shift = DVec3::new(-profile.centre[0], -profile.centre[1], -z0);
    (DMat4::from_translation(shift) * DMat4::from_quat(rot)).to_cols_array_2d()
}

/// The largest radius about the rotated z axis, slice by slice along it.
struct Profile {
    /// The rotated points.
    pts: Vec<DVec3>,
    /// Where the axis is, in rotated x and y.
    centre: [f64; 2],
    /// The bottom of the first slice, and each slice's height.
    z0: f64,
    slice: f64,
    radius: Vec<f64>,
}

impl Profile {
    const SLICES: usize = 512;

    fn new(pts: &[DVec3], indices: &[u32], rot: DQuat) -> Self {
        let pts: Vec<DVec3> = pts.iter().map(|&p| rot * p).collect();
        let (lo, hi) = pts.iter().fold(
            (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let slice = (hi.z - lo.z).max(1e-9) / Self::SLICES as f64;
        let mut profile = Self {
            pts,
            centre: [(lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0],
            z0: lo.z,
            slice,
            radius: vec![0.0; Self::SLICES],
        };
        let index = |z: f64| (((z - lo.z) / slice) as usize).min(Self::SLICES - 1);
        // A triangle covers the slices between its lowest and highest corner, out to its
        // farthest corner: right for the cylinders and flat steps turned parts are made of.
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| profile.pts[t[k] as usize]);
            let r = profile.r(&a).max(profile.r(&b)).max(profile.r(&c));
            let (bottom, top) = (a.z.min(b.z).min(c.z), a.z.max(b.z).max(c.z));
            for s in &mut profile.radius[index(bottom)..=index(top)] {
                *s = f64::max(*s, r);
            }
        }
        // Gaps between separate parts take the radius below them, so they are not steps.
        for i in 1..Self::SLICES {
            if profile.radius[i] == 0.0 {
                profile.radius[i] = profile.radius[i - 1];
            }
        }
        profile
    }

    /// Distance from the axis.
    fn r(&self, p: &DVec3) -> f64 {
        (p.x - self.centre[0]).hypot(p.y - self.centre[1])
    }
}

/// Wraps the default scene of a `.glb` in one new root node that applies `m` (column-major,
/// model space) and records `alignment` as its `extras.hydra_alignment`. The binary chunk is
/// kept as it is.
pub fn set_root_transform(
    glb: &[u8],
    m: [[f64; 4]; 4],
    alignment: serde_json::Value,
) -> Result<Vec<u8>, String> {
    use serde_json::{Value, json};
    let parsed = gltf::binary::Glb::from_slice(glb)
        .map_err(|e| format!("not a binary glTF (.glb) file: {e}"))?;
    let mut doc: Value = serde_json::from_slice(&parsed.json)
        .map_err(|e| format!("the glb file's JSON can't be read: {e}"))?;
    // The scene load_glb draws.
    let scene = doc.get("scene").and_then(Value::as_u64).unwrap_or(0);
    let roots = doc
        .pointer_mut(&format!("/scenes/{scene}/nodes"))
        .ok_or("the glTF file has no scene")?
        .take();
    let nodes = doc
        .as_object_mut()
        .ok_or("the glb file's JSON is not an object")?
        .entry("nodes")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("the glb file's nodes are not a list")?;
    // load_glb maps glTF space to model space with FROM_GLTF, so conjugate by it.
    let from = FROM_GLTF.as_dmat4();
    let matrix = from.inverse() * DMat4::from_cols_array_2d(&m) * from;
    nodes.push(json!({
        "name": "hydra-alignment",
        "matrix": matrix.to_cols_array(),
        "children": roots,
        "extras": {"hydra_alignment": alignment},
    }));
    let root = nodes.len() - 1;
    *doc.pointer_mut(&format!("/scenes/{scene}/nodes")).unwrap() = json!([root]);
    let json = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    gltf::binary::Glb {
        json: json.into(),
        ..parsed
    }
    .to_vec()
    .map_err(|e| e.to_string())
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

    /// `mesh` moved by the column-major `m`.
    fn moved(mesh: &Mesh, m: [[f64; 4]; 4]) -> Mesh {
        let m = DMat4::from_cols_array_2d(&m);
        Mesh {
            positions: (mesh.positions.iter())
                .map(|&p| {
                    m.transform_point3(Vec3::from(p).as_dvec3())
                        .as_vec3()
                        .into()
                })
                .collect(),
            ..mesh.clone()
        }
    }

    /// Adds a closed cylinder along x from `x0` to `x1`, its axis at (y, z) = `at`.
    fn cylinder(mesh: &mut Mesh, x0: f32, x1: f32, r: f32, at: [f32; 2]) {
        const N: u32 = 48;
        let base = mesh.positions.len() as u32;
        for x in [x0, x1] {
            for k in 0..N {
                let a = k as f32 * std::f32::consts::TAU / N as f32;
                mesh.positions
                    .push([x, at[0] + r * a.cos(), at[1] + r * a.sin()]);
                mesh.normals.push([0.0, a.cos(), a.sin()]);
            }
        }
        for k in 0..N {
            let (a, b) = (base + k, base + (k + 1) % N);
            mesh.indices.extend([a, b, b + N, a, b + N, a + N]);
            // Caps as fans from each ring's first vertex.
            if k > 0 && k + 1 < N {
                mesh.indices.extend([base, a, b, base + N, b + N, a + N]);
            }
        }
    }

    #[test]
    fn a_cartridge_lying_along_x_stands_up_on_its_shoulder() {
        // A wide 30 mm head (x 0..20) on a 16 mm stem (x 20..60), axis off-centre.
        let mut mesh = Mesh::default();
        cylinder(&mut mesh, 0.0, 20.0, 15.0, [5.0, -3.0]);
        cylinder(&mut mesh, 20.0, 60.0, 8.0, [5.0, -3.0]);
        mesh.check().unwrap();
        let (lo, hi) = moved(&mesh, guess_alignment(&mesh)).bounds();
        let near = |a: f32, b: f32| (a - b).abs() < 0.5;
        // Stem down, head up, the shoulder at z = 0, on the z axis.
        assert!(near(lo[2], -40.0) && near(hi[2], 20.0), "z {lo:?}..{hi:?}");
        assert!(near(lo[0], -15.0) && near(hi[0], 15.0), "x {lo:?}..{hi:?}");
        assert!(near(lo[1], -15.0) && near(hi[1], 15.0), "y {lo:?}..{hi:?}");

        // A plain cylinder has no shoulder: it stands on its bottom.
        let mut plain = Mesh::default();
        cylinder(&mut plain, 0.0, 60.0, 8.0, [0.0, 0.0]);
        let (lo, hi) = moved(&plain, guess_alignment(&plain)).bounds();
        assert!(near(lo[2], 0.0) && near(hi[2], 60.0), "z {lo:?}..{hi:?}");
    }

    #[test]
    fn the_demo_cartridge_stands_on_its_seat_however_it_lies() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/models/demo-cartridge.glb"
        );
        let mesh = load_glb(&std::fs::read(path).unwrap()).unwrap();
        let lying = [
            DMat4::IDENTITY,
            DMat4::from_rotation_x(1.0),
            DMat4::from_translation(DVec3::new(30.0, -7.0, 12.0)) * DMat4::from_rotation_y(-2.5),
            DMat4::from_rotation_z(0.7) * DMat4::from_rotation_x(std::f64::consts::PI),
        ];
        for m in lying {
            let mesh = moved(&mesh, m.to_cols_array_2d());
            let (lo, hi) = moved(&mesh, guess_alignment(&mesh)).bounds();
            // Thread and nose 55 mm below the seat, hex and adjuster 55 mm above.
            let near = |a: f32, b: f32| (a - b).abs() < 0.05;
            assert!(near(lo[2], -55.0) && near(hi[2], 55.0), "z {lo:?}..{hi:?}");
            assert!(near(lo[0], -hi[0]) && near(lo[1], -hi[1]), "{lo:?}..{hi:?}");
        }
    }

    #[test]
    fn a_root_transform_moves_the_model_and_keeps_its_note() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/models/demo-valve-cetop3.glb"
        );
        let glb = std::fs::read(path).unwrap();
        // Turn 90 degrees about z, then move 10 mm along x: (x, y, z) -> (10 - y, x, z).
        let m = (DMat4::from_translation(DVec3::new(10.0, 0.0, 0.0))
            * DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2))
        .to_cols_array_2d();
        let note = serde_json::json!({"rotate": ["z:90"], "offset": [10, 0, 0]});
        let out = set_root_transform(&glb, m, note.clone()).unwrap();
        assert_eq!(out.len() % 4, 0);
        let (lo, hi) = load_glb(&out).unwrap().bounds();
        let near = |a: f32, b: f32| (a - b).abs() < 0.5;
        // The valve spans x -49.5..90, y -6..38, z 0..52.
        assert!(near(lo[0], -28.0) && near(hi[0], 16.0), "x {lo:?}..{hi:?}");
        assert!(near(lo[1], -49.5) && near(hi[1], 90.0), "y {lo:?}..{hi:?}");
        assert!(near(lo[2], 0.0) && near(hi[2], 52.0), "z {lo:?}..{hi:?}");

        let parsed = gltf::binary::Glb::from_slice(&out).unwrap();
        assert_eq!(parsed.bin, gltf::binary::Glb::from_slice(&glb).unwrap().bin);
        let doc: serde_json::Value = serde_json::from_slice(&parsed.json).unwrap();
        let roots = doc["scenes"][0]["nodes"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        let root = &doc["nodes"][roots[0].as_u64().unwrap() as usize];
        assert_eq!(root["extras"]["hydra_alignment"], note);
        assert!(set_root_transform(b"not a model", m, note).is_err());
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
