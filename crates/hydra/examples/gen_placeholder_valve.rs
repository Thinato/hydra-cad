//! Writes `assets/valve-cetop3-placeholder.glb`: a flat-shaded stand-in for a
//! CETOP 3 / NG6 directional valve (body, two solenoids, two connectors).
//! Units are metres, Y up, long axis along X, mounting face at y = 0.
//!
//! Run: `cargo run -p hydra --example gen_placeholder_valve`

use serde_json::json;
use std::f32::consts::TAU;

#[derive(Default)]
struct Builder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Builder {
    /// Adds a convex polygon (counter-clockwise seen from outside) with one normal.
    fn face(&mut self, verts: &[[f32; 3]], n: [f32; 3]) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(verts);
        self.normals.extend(std::iter::repeat_n(n, verts.len()));
        for i in 1..verts.len() as u32 - 1 {
            self.indices.extend([base, base + i, base + i + 1]);
        }
    }

    /// Axis-aligned box from `min` to `max`.
    fn cuboid(&mut self, min: [f32; 3], max: [f32; 3]) {
        let c = [0, 1, 2].map(|i| (min[i] + max[i]) / 2.0);
        let h = [0, 1, 2].map(|i| (max[i] - min[i]) / 2.0);
        // (normal axis, u axis, v axis) with u x v = +normal axis.
        for (a, u, v) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            for s in [1.0f32, -1.0] {
                let corner = |su: f32, sv: f32| {
                    let mut p = c;
                    p[a] += s * h[a];
                    p[u] += su * h[u];
                    p[v] += s * sv * h[v]; // flip v on the negative side to keep CCW
                    p
                };
                let mut n = [0.0; 3];
                n[a] = s;
                self.face(
                    &[
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ],
                    n,
                );
            }
        }
    }

    /// Low-poly cylinder along X from `x0` to `x1`, centred on (y, z).
    fn cylinder_x(&mut self, x0: f32, x1: f32, y: f32, z: f32, r: f32, segments: usize) {
        let ring = |i: usize| {
            let a = TAU * i as f32 / segments as f32;
            (y + r * a.cos(), z + r * a.sin())
        };
        for i in 0..segments {
            let ((y0, z0), (y1, z1)) = (ring(i), ring(i + 1));
            let mid = TAU * (i as f32 + 0.5) / segments as f32;
            self.face(
                &[[x0, y0, z0], [x0, y1, z1], [x1, y1, z1], [x1, y0, z0]],
                [0.0, mid.cos(), mid.sin()],
            );
        }
        let cap: Vec<[f32; 3]> = (0..segments)
            .map(ring)
            .map(|(cy, cz)| [x1, cy, cz])
            .collect();
        self.face(&cap, [1.0, 0.0, 0.0]);
        let cap: Vec<[f32; 3]> = cap.iter().rev().map(|p| [x0, p[1], p[2]]).collect();
        self.face(&cap, [-1.0, 0.0, 0.0]);
    }
}

fn main() -> anyhow::Result<()> {
    let mut b = Builder::default();
    // Body 75 x 45 x 45 mm.
    b.cuboid([-0.0375, 0.0, -0.0225], [0.0375, 0.045, 0.0225]);
    for s in [1.0f32, -1.0] {
        // Solenoid coil, 45 mm long, 34 mm diameter.
        let (x0, x1) = (s * 0.0375, s * 0.0825);
        b.cylinder_x(x0.min(x1), x0.max(x1), 0.0255, 0.0, 0.017, 16);
        // Connector box on top of the coil.
        let (cx0, cx1) = (s * 0.048, s * 0.078);
        b.cuboid([cx0.min(cx1), 0.040, -0.014], [cx0.max(cx1), 0.058, 0.014]);
    }

    let (min, max) =
        b.positions
            .iter()
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(mut lo, mut hi), p| {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
                (lo, hi)
            });

    let mut bin = Vec::new();
    for p in b.positions.iter().chain(&b.normals) {
        for v in p {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    for i in &b.indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let vcount = b.positions.len();
    let vbytes = vcount * 12;

    let doc = json!({
        "asset": { "version": "2.0", "generator": "hydra gen_placeholder_valve" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "name": "valve-cetop3-placeholder", "mesh": 0 }],
        "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "NORMAL": 1 },
            "indices": 2,
            "material": 0
        }]}],
        "materials": [{
            "name": "valve-body",
            "pbrMetallicRoughness": {
                "baseColorFactor": [0.55, 0.6, 0.68, 1.0],
                "metallicFactor": 0.3,
                "roughnessFactor": 0.6
            }
        }],
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": vbytes, "target": 34962 },
            { "buffer": 0, "byteOffset": vbytes, "byteLength": vbytes, "target": 34962 },
            { "buffer": 0, "byteOffset": 2 * vbytes, "byteLength": b.indices.len() * 4, "target": 34963 }
        ],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": vcount, "type": "VEC3", "min": min, "max": max },
            { "bufferView": 1, "componentType": 5126, "count": vcount, "type": "VEC3" },
            { "bufferView": 2, "componentType": 5125, "count": b.indices.len(), "type": "SCALAR" }
        ]
    });

    let mut json = serde_json::to_vec(&doc)?;
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);

    let out = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/valve-cetop3-placeholder.glb"
    );
    std::fs::create_dir_all(std::path::Path::new(out).parent().unwrap())?;
    std::fs::write(out, &glb)?;
    println!(
        "wrote {out}: {vcount} vertices, {} triangles",
        b.indices.len() / 3
    );
    Ok(())
}
