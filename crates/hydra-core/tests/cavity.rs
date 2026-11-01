//! A cartridge cavity is a mount face: its frame puts a cartridge model into the block.

use hydra_core::{Catalogue, CatalogueRef};

#[test]
fn the_cavity_frame_puts_the_thread_inside_the_block() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
    let c: Catalogue = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let block = c.block(&CatalogueRef::new("blk-demo", 2)).unwrap();
    let cartridge = c
        .valve_type(&CatalogueRef::new("vt-demo-cartridge-relief", 1))
        .unwrap();
    let face = block.face("C1").unwrap();
    assert_eq!(cartridge.mounting.as_deref(), Some(face.pattern.as_str()));

    // A point on the cartridge axis 10 mm below its seating face lands 10 mm into the block
    // along the cavity axis, from the +x face.
    let m = face.transform().unwrap();
    let p = [0.0, 0.0, -10.0, 1.0];
    let mapped: Vec<f64> = (0..3)
        .map(|row| (0..4).map(|col| m[col][row] * p[col]).sum())
        .collect();
    for (got, want) in mapped.iter().zip([120.0, 55.0, 35.0]) {
        assert!((got - want).abs() < 1e-9, "{mapped:?}");
    }
}
