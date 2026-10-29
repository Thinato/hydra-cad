//! Symbol snapshots and golden circuits, from the repo's fixtures/ directory.

use std::path::{Path, PathBuf};

use hydra_core::{Catalogue, Circuit, Severity};
use hydra_symbols::{Primitive, bounds, render_circuit, render_symbol, to_svg};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn catalogue() -> Catalogue {
    let json = std::fs::read_to_string(fixtures().join("catalogue.json")).unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
fn fixture_catalogue_is_consistent() {
    assert_eq!(catalogue().problems(), Vec::<String>::new());
}

#[test]
fn every_catalogue_symbol_matches_its_snapshot() {
    for record in catalogue().symbol_defs {
        let drawing = render_symbol(&record.definition);
        let mut names: Vec<_> = drawing.ports.iter().map(|(n, _)| n.as_str()).collect();
        let mut expected = record.definition.ports();
        names.sort();
        expected.sort();
        assert_eq!(names, expected, "{} anchors every port", record.id);
        insta::assert_snapshot!(record.id.clone(), to_svg(&drawing.primitives));
    }
}

#[test]
fn golden_circuits_load_validate_round_trip_and_render() {
    let catalogue = catalogue();
    let mut count = 0;
    for entry in std::fs::read_dir(fixtures().join("circuits")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let circuit = Circuit::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let issues = circuit.validate(&catalogue);
        assert!(issues.is_empty(), "{name}: {issues:?}");
        assert_eq!(
            Circuit::from_json(&circuit.to_json()).unwrap(),
            circuit,
            "{name} round-trips"
        );
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&circuit.to_json()).unwrap();
        if let Err(at) = kept(&file, &saved, String::new()) {
            panic!("{name}: {at} is dropped or changed on save");
        }

        let drawing = render_circuit(&circuit, &catalogue);
        assert!(
            bounds(&drawing.primitives).is_some(),
            "{name} draws something"
        );
        insta::assert_snapshot!(format!("circuit-{name}"), to_svg(&drawing.primitives));
    }
    assert!(count > 0, "no golden circuits found");
}

/// Every value in `file` survives in `saved`; `saved` may add defaults.
fn kept(file: &serde_json::Value, saved: &serde_json::Value, at: String) -> Result<(), String> {
    use serde_json::Value::{Array, Number, Object};
    match (file, saved) {
        (Object(f), Object(s)) => f.iter().try_for_each(|(k, v)| {
            let at = format!("{at}/{k}");
            kept(v, s.get(k).ok_or(at.clone())?, at)
        }),
        (Array(f), Array(s)) if f.len() == s.len() => f
            .iter()
            .zip(s)
            .enumerate()
            .try_for_each(|(i, (f, s))| kept(f, s, format!("{at}/{i}"))),
        (Number(f), Number(s)) if f.as_f64() == s.as_f64() => Ok(()),
        (f, s) if f == s => Ok(()),
        _ => Err(at),
    }
}

#[test]
fn rotation_moves_port_anchors_clockwise() {
    let catalogue = catalogue();
    let json = r#"{"schema": 1, "name": "r", "components": [
        {"id": "K", "kind": "valve", "valve_type": "vt-demo-check"}],
        "layout": {"components": {"K": {"x": 100, "y": 50, "rotation": 90}}}}"#;
    let circuit = Circuit::from_json(json).unwrap();
    let drawing = render_circuit(&circuit, &catalogue);
    let anchor = |name: &str| drawing.ports.iter().find(|(n, _)| n == name).unwrap().1;
    // Unrotated, A is below the origin and B above; a quarter turn clockwise puts A left, B right.
    assert_eq!(anchor("K.A"), [92.0, 50.0]);
    assert_eq!(anchor("K.B"), [108.0, 50.0]);
    let issues = circuit.validate(&catalogue);
    assert_eq!(issues.len(), 2);
    assert!(issues.iter().all(|i| i.severity() == Severity::Warning));
}

#[test]
fn nets_are_drawn_between_port_anchors() {
    let catalogue = catalogue();
    let json = r#"{"schema": 1, "name": "w", "components": [
        {"id": "X", "kind": "port", "label": "X"}, {"id": "Y", "kind": "port", "label": "Y"}],
        "nets": [{"id": "n", "ports": [{"component": "X", "port": "port"}, {"component": "Y", "port": "port"}]}],
        "layout": {"components": {"X": {"x": 0, "y": 0}, "Y": {"x": 10, "y": 5}}}}"#;
    let drawing = render_circuit(&Circuit::from_json(json).unwrap(), &catalogue);
    let wire = Primitive::Polyline {
        points: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]],
        closed: false,
        dashed: false,
        filled: false,
    };
    assert!(drawing.primitives.contains(&wire));
}
