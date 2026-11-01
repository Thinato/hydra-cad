//! Symbol snapshots and golden circuits, from the repo's fixtures/ directory.

use std::path::{Path, PathBuf};

use hydra_core::{Catalogue, Circuit, SCHEMA_VERSION, Severity};
use hydra_symbols::{
    Align, Primitive, block_outline, bounds, outline_issues, render_circuit, render_symbol, to_pdf,
    to_svg,
};

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
        let misplaced = outline_issues(&circuit, &catalogue);
        assert!(misplaced.is_empty(), "{name}: {misplaced:?}");
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
fn mirror_flips_a_symbol_left_to_right_before_turning_it() {
    let catalogue = catalogue();
    let relief = |rotation: u16, mirror: bool| {
        let json = format!(
            r#"{{"schema": 4, "name": "m", "components": [
            {{"id": "R", "kind": "valve", "valve_type": "vt-demo-cartridge-relief@1"}}],
            "layout": {{"components": {{"R": {{"x": 0, "y": 0, "rotation": {rotation}, "mirror": {mirror}}}}}}}}}"#
        );
        render_circuit(&Circuit::from_json(&json).unwrap(), &catalogue)
    };
    let symbol = catalogue
        .symbol_of(&hydra_core::CatalogueRef::new(
            "vt-demo-cartridge-relief",
            1,
        ))
        .unwrap();
    let n = render_symbol(symbol).primitives.len();
    let (plain, flipped) = (relief(0, false), relief(0, true));
    // Every line and circle of the symbol lands on the other side; text keeps reading forwards.
    for (a, b) in plain.primitives[..n].iter().zip(&flipped.primitives[..n]) {
        match (a, b) {
            (Primitive::Polyline { points: p, .. }, Primitive::Polyline { points: q, .. }) => {
                let back: Vec<_> = q.iter().map(|[x, y]| [-x, *y]).collect();
                assert_eq!(p, &back);
            }
            (Primitive::Circle { center: c, .. }, Primitive::Circle { center: d, .. }) => {
                assert_eq!(*c, [-d[0], d[1]]);
            }
            (
                Primitive::Text { at, align, .. },
                Primitive::Text {
                    at: at2,
                    align: align2,
                    ..
                },
            ) => {
                assert_eq!(*at, [-at2[0], at2[1]]);
                let swapped = match align {
                    Align::Start => Align::End,
                    Align::End => Align::Start,
                    Align::Middle => Align::Middle,
                };
                assert_eq!(*align2, swapped);
            }
            _ => panic!("{a:?} / {b:?}"),
        }
    }
    // Mirrored, then a quarter turn: P on the left, T on the right, the spring on top.
    let turned = relief(90, true);
    let anchor = |name: &str| turned.ports.iter().find(|(n, _)| n == name).unwrap().1;
    assert!(anchor("R.P")[0] < 0.0 && anchor("R.T")[0] > 0.0);
    let (lo, hi) = bounds(&turned.primitives[..n]).unwrap();
    let (plain_lo, plain_hi) = bounds(&relief(90, false).primitives[..n]).unwrap();
    assert!(
        -lo[1] > hi[1] && plain_hi[1] > -plain_lo[1],
        "{lo:?} {hi:?}"
    );
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

#[test]
fn block_outline_goes_round_its_parts_through_its_ports() {
    let catalogue = catalogue();
    let json = r#"{"schema": 4, "name": "b", "components": [
        {"id": "P", "kind": "port", "label": "P"},
        {"id": "X1", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "X2", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "X3", "kind": "item", "symbol": {"kind": "orifice"}}],
        "layout": {"components": {"P": {"x": 0, "y": 60}, "X1": {"x": 30, "y": 30},
                                  "X2": {"x": 20, "y": 45}, "X3": {"x": 100, "y": 30}}},
        "in_block": ["P", "X1"]}"#;
    let mut circuit = Circuit::from_json(json).unwrap();
    let (min, max) = bounds_of(&block_outline(&circuit, &catalogue).unwrap());
    // The port beyond the parts sits on the edge; the part has room inside.
    assert_eq!((min[0], max[1]), (0.0, 60.0));
    let x1 = render_circuit(
        &Circuit {
            in_block: Default::default(),
            components: vec![circuit.components[1].clone()],
            ..circuit.clone()
        },
        &catalogue,
    );
    let (lo, hi) = bounds(&x1.primitives).unwrap();
    assert!(
        min[1] < lo[1] - 3.0 && hi[0] + 3.0 < max[0],
        "{min:?} {max:?}"
    );
    // X2 is drawn inside, beside X1, but isn't in the block; X3 is clear of it.
    let issues: Vec<String> = outline_issues(&circuit, &catalogue)
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert_eq!(
        issues,
        [
            "X2 is outside the block but drawn inside its outline; move it out or put it in the block"
        ]
    );

    // The outline is drawn as dash-dot segments on its four edges.
    let with = render_circuit(&circuit, &catalogue).primitives;
    circuit.in_block.clear();
    let without = render_circuit(&circuit, &catalogue).primitives;
    let added = &with[without.len()..];
    assert!(!added.is_empty());
    // Starting at the top-left corner: a 4 mm dash, a 1 mm gap, a 0.3 mm dot, a 1 mm gap.
    let first: Vec<_> = added[..3]
        .iter()
        .map(|p| match p {
            Primitive::Polyline { points, .. } => points.clone(),
            _ => panic!("{p:?}"),
        })
        .collect();
    let [x, y] = min;
    let close = |a: &[[f32; 2]], b: [[f32; 2]; 2]| {
        a.len() == 2
            && a.iter()
                .zip(b)
                .all(|(p, q)| (p[0] - q[0]).abs() < 1e-3 && p[1] == q[1])
    };
    assert!(close(&first[0], [[x, y], [x + 4.0, y]]), "{first:?}");
    assert!(close(&first[1], [[x + 5.0, y], [x + 5.3, y]]), "{first:?}");
    assert!(close(&first[2], [[x + 6.3, y], [x + 10.3, y]]), "{first:?}");
    let on_edge = |[x, y]: [f32; 2]| {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        (near(x, min[0]) || near(x, max[0])) && (min[1]..=max[1]).contains(&y)
            || (near(y, min[1]) || near(y, max[1])) && (min[0]..=max[0]).contains(&x)
    };
    for p in added {
        let Primitive::Polyline { points, .. } = p else {
            panic!("{p:?}")
        };
        assert!(points.iter().all(|q| on_edge(*q)), "{points:?}");
    }
    assert!(outline_issues(&circuit, &catalogue).is_empty());

    // A part's drawing reaching over the outline counts, not only its origin.
    circuit.in_block = ["P".to_string(), "X1".to_string()].into();
    let x2 = circuit.layout.components.get_mut("X2").unwrap();
    (x2.x, x2.y) = (5.0, 60.0 + 3.0);
    let issues = outline_issues(&circuit, &catalogue);
    assert_eq!(issues, [hydra_core::Issue::DrawnInsideBlock("X2".into())]);

    // A block of one port is a point, and draws nothing.
    circuit.in_block = ["P".to_string()].into();
    assert_eq!(block_outline(&circuit, &catalogue), Some(vec![[0.0, 60.0]]));
    assert_eq!(
        render_circuit(&circuit, &catalogue).primitives.len(),
        without.len()
    );

    // A part placed far off by mistake gets one dashed outline, not millions of dashes.
    circuit.in_block = ["P".to_string(), "X3".to_string()].into();
    circuit.layout.components.get_mut("X3").unwrap().x = 2e8;
    let far = render_circuit(&circuit, &catalogue).primitives;
    assert_eq!(far.len(), without.len() + 1);
    assert!(matches!(
        far.last(),
        Some(Primitive::Polyline {
            dashed: true,
            closed: true,
            ..
        })
    ));
}

/// The outline's polylines, which `render_circuit` appends after everything else.
fn outline_lines(circuit: &Circuit, catalogue: &Catalogue) -> Vec<[[f32; 2]; 2]> {
    let with = render_circuit(circuit, catalogue).primitives;
    let without = render_circuit(
        &Circuit {
            in_block: Default::default(),
            ..circuit.clone()
        },
        catalogue,
    )
    .primitives;
    with[without.len()..]
        .iter()
        .map(|p| match p {
            Primitive::Polyline { points, .. } if points.len() == 2 => [points[0], points[1]],
            _ => panic!("{p:?}"),
        })
        .collect()
}

#[test]
fn an_outside_part_in_an_empty_corner_gets_a_notch() {
    let catalogue = catalogue();
    // Block parts A, B, C make an L; N stands outside the block in its top-right notch, and M
    // outside it between A and B.
    let json = r#"{"schema": 4, "name": "l", "components": [
        {"id": "A", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "B", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "C", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "N", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "M", "kind": "item", "symbol": {"kind": "orifice"}}],
        "layout": {"components": {"A": {"x": 0, "y": 0}, "B": {"x": 0, "y": 40},
            "C": {"x": 40, "y": 40}, "N": {"x": 40, "y": 0}, "M": {"x": 0, "y": 20}}},
        "in_block": ["A", "B", "C"]}"#;
    let mut circuit = Circuit::from_json(json).unwrap();
    let only = |id: &str| {
        let mut c = circuit.clone();
        c.components.retain(|c| c.id == id);
        c.in_block.clear();
        bounds(&render_circuit(&c, &catalogue).primitives).unwrap()
    };
    let (n, a, c) = (only("N"), only("A"), only("C"));
    let (min, max) = ([a.0[0] - 4.0, a.0[1] - 4.0], [c.1[0] + 4.0, c.1[1] + 4.0]);
    // The notch leaves room round N, as the outline does round the block's parts.
    let inner = [n.0[0] - 4.0, n.1[1] + 4.0];
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            min,
            [inner[0], min[1]],
            inner,
            [max[0], inner[1]],
            max,
            [min[0], max[1]]
        ]
    );
    // N in the notch is fine; M between two block parts is drawn inside the block.
    assert_eq!(
        outline_issues(&circuit, &catalogue),
        [hydra_core::Issue::DrawnInsideBlock("M".into())]
    );
    // The drawn outline is that same shape: every dash lies on one of its edges.
    let corners = block_outline(&circuit, &catalogue).unwrap();
    let on_edge = |p: [f32; 2]| {
        (0..corners.len()).any(|k| {
            let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
            let within = |v: f32, a: f32, b: f32| a.min(b) - 1e-3 <= v && v <= a.max(b) + 1e-3;
            within(p[0], a[0], b[0]) && within(p[1], a[1], b[1])
        })
    };
    let lines = outline_lines(&circuit, &catalogue);
    assert!(lines.len() > 20);
    assert!(lines.iter().flatten().all(|&p| on_edge(p)), "{lines:?}");
    // A part added in the notch would be drawn outside, one by M inside.
    let n_part = circuit.component("N").unwrap().clone();
    let place = |x, y| hydra_core::Placement {
        x,
        y,
        ..Default::default()
    };
    assert!(!hydra_symbols::drawn_inside_outline(
        &circuit,
        &n_part,
        place(40.0, 0.0),
        &catalogue
    ));
    assert!(hydra_symbols::drawn_inside_outline(
        &circuit,
        &n_part,
        place(0.0, 20.0),
        &catalogue
    ));

    // With nothing in the corner the block stays a rectangle.
    circuit.components.retain(|c| c.id != "N");
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [min, [max[0], min[1]], max, [min[0], max[1]]]
    );
}

#[test]
fn block_port_labels_sit_outside_and_the_outline_breaks_round_ports() {
    let catalogue = catalogue();
    let json = r#"{"schema": 4, "name": "p", "components": [
        {"id": "X", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "T", "kind": "port", "label": "TOP"}, {"id": "B", "kind": "port", "label": "BOTTOM"},
        {"id": "L", "kind": "port", "label": "LEFT"}, {"id": "R", "kind": "port", "label": "RIGHT"}],
        "layout": {"components": {"X": {"x": 0, "y": 0}, "T": {"x": 0, "y": -30},
            "B": {"x": 5, "y": 30}, "L": {"x": -30, "y": 3}, "R": {"x": 30, "y": -3}}},
        "in_block": ["X", "T", "B", "L", "R"]}"#;
    let circuit = Circuit::from_json(json).unwrap();
    let (min, max) = bounds_of(&block_outline(&circuit, &catalogue).unwrap());
    assert_eq!((min, max), ([-30.0, -30.0], [30.0, 30.0]));
    let primitives = render_circuit(&circuit, &catalogue).primitives;
    for label in ["TOP", "BOTTOM", "LEFT", "RIGHT"] {
        let text = primitives
            .iter()
            .find(|p| matches!(p, Primitive::Text { text, .. } if text == label))
            .unwrap();
        let (lo, hi) = bounds(std::slice::from_ref(text)).unwrap();
        let inside = lo[0] < max[0] && min[0] < hi[0] && lo[1] < max[1] && min[1] < hi[1];
        assert!(!inside, "{label} at {lo:?}..{hi:?} is inside the block");
    }
    // No dash runs into a port's circle.
    let lines = outline_lines(&circuit, &catalogue);
    for port in [[0.0, -30.0], [5.0, 30.0], [-30.0, 3.0], [30.0, -3.0]] {
        let near = |[a, b]: &[[f32; 2]; 2]| {
            let clamp = |v: f32, a: f32, b: f32| v.clamp(a.min(b), a.max(b));
            let q = [clamp(port[0], a[0], b[0]), clamp(port[1], a[1], b[1])];
            (q[0] - port[0]).hypot(q[1] - port[1]) < 1.2 - 1e-3
        };
        assert!(
            !lines.iter().any(near),
            "the outline crosses the port at {port:?}"
        );
    }
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

/// The box round an outline's corners.
fn bounds_of(corners: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    corners.iter().fold(
        ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
        |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        },
    )
}
