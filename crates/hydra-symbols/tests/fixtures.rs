//! Symbol snapshots and golden circuits, from the repo's fixtures/ directory.

use std::path::{Path, PathBuf};

use hydra_core::{Catalogue, Circuit, SCHEMA_VERSION, Severity};
use hydra_symbols::{Align, Primitive, bounds, render_circuit, render_symbol, to_pdf, to_svg};

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
        assert_eq!(names, expected, "{} anchors every port", record.key);
        insta::assert_snapshot!(record.key.clone(), to_svg(&drawing.primitives));
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
        let mut file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Older documents are upgraded on load and saved as the current version.
        file["schema"] = SCHEMA_VERSION.into();
        let saved: serde_json::Value = serde_json::from_str(&circuit.to_json()).unwrap();
        if let Err(at) = kept(&file, &saved, String::new()) {
            panic!("{name}: {at} is dropped or changed on save");
        }

        let drawing = render_circuit(&circuit, &catalogue);
        assert!(
            bounds(&drawing.primitives).is_some(),
            "{name} draws something"
        );
        let svg = to_svg(&drawing.primitives);
        insta::assert_snapshot!(format!("circuit-{name}"), svg);
        insta::assert_snapshot!(
            format!("circuit-{name}-bom"),
            hydra_core::bom_csv(&circuit.bom(&catalogue))
        );

        let pdf = to_pdf(&drawing.primitives).unwrap();
        assert!(pdf.starts_with(b"%PDF-"), "{name}: not a PDF");
        assert!(pdf.len() < 200_000, "{name}: PDF is {} bytes", pdf.len());
        let (w, h) = media_box(&pdf);
        let (sw, sh) = (svg_mm(&svg, "width"), svg_mm(&svg, "height"));
        let pt = 72.0 / 25.4;
        assert!(
            (w - sw * pt).abs() < 0.5 && (h - sh * pt).abs() < 0.5,
            "{name}: page {w}x{h}pt, drawing {sw}x{sh}mm"
        );
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
        {"id": "K", "kind": "valve", "valve_type": "vt-demo-check@1"}],
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

/// Page size in points from the first `/MediaBox [0 0 w h]`.
fn media_box(pdf: &[u8]) -> (f32, f32) {
    let text = String::from_utf8_lossy(pdf);
    let rest = &text[text.find("/MediaBox").expect("a MediaBox") + 9..];
    let inner = &rest[rest.find('[').unwrap() + 1..rest.find(']').unwrap()];
    let n: Vec<f32> = inner
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    (n[2] - n[0], n[3] - n[1])
}

/// A millimetre attribute of the root `<svg>` element.
fn svg_mm(svg: &str, attr: &str) -> f32 {
    let rest = &svg[svg.find(&format!(" {attr}=\"")).unwrap() + attr.len() + 3..];
    rest[..rest.find("mm").unwrap()].parse().unwrap()
}

#[test]
fn pdf_keeps_text_as_a_font() {
    let pdf = to_pdf(&[Primitive::Text {
        at: [5.0, 5.0],
        text: "RV1".into(),
        size: 3.0,
        align: Align::Start,
    }])
    .unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("/FontFile") || text.contains("/Type0"),
        "no embedded font"
    );
}

#[test]
fn generic_symbols_anchor_every_port_and_match_their_snapshots() {
    let defs = [
        ("test-point", r#"{"kind": "test_point"}"#),
        ("hydraulic-motor", r#"{"kind": "hydraulic_motor"}"#),
        (
            "hydraulic-motor-reversible-variable-drain",
            r#"{"kind": "hydraulic_motor", "bidirectional": true, "variable": true, "drain": true}"#,
        ),
        ("pump", r#"{"kind": "pump"}"#),
        ("pump-variable", r#"{"kind": "pump", "variable": true}"#),
        (
            "box",
            r#"{"kind": "box", "label": "PB10", "ports": ["1", "2", "3"]}"#,
        ),
        ("flow-control-manual", r#"{"kind": "flow_control"}"#),
    ];
    for (name, json) in defs {
        let def: hydra_core::SymbolDef = serde_json::from_str(json).unwrap();
        let drawing = render_symbol(&def);
        let mut names: Vec<_> = drawing.ports.iter().map(|(n, _)| n.as_str()).collect();
        let mut expected = def.ports();
        names.sort();
        expected.sort();
        assert_eq!(names, expected, "{name} anchors every port");
        insta::assert_snapshot!(format!("generic-{name}"), to_svg(&drawing.primitives));
    }
}

#[test]
fn statuses_and_item_labels_are_written_out() {
    let json =
        std::fs::read_to_string(fixtures().join("circuits/demo-import-manifold.json")).unwrap();
    let circuit = Circuit::from_json(&json).unwrap();
    let drawing = render_circuit(&circuit, &catalogue());
    let svg = to_svg(&drawing.primitives);
    for words in [
        "MISSING DEMO-10-4-3-E-24DC",
        "MISSING DEMO-PB10",
        "PART NOT LISTED DEMO-06-4-3-T-24DC",
        "STAND-IN for DEMO-CK10-05",
        "X 0.5 mm",
        "M SAE-04",
    ] {
        assert!(svg.contains(&format!(">{words}<")), "{words}");
    }
    // Normal valves carry no status words.
    assert!(!svg.contains("DEMO-06-4-2-S-24DC<"));
    let frames = drawing
        .primitives
        .iter()
        .filter(|p| {
            matches!(
                p,
                Primitive::Polyline {
                    closed: true,
                    dashed: true,
                    ..
                }
            )
        })
        .count();
    // 1.1 and 5.1 not listed, 4.1, 5.2 and 9.1 missing, 3.1 a stand-in.
    assert_eq!(frames, 6);
    assert!(to_pdf(&drawing.primitives).is_ok());
}
