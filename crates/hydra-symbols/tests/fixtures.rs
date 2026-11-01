//! Symbol snapshots and golden circuits, from the repo's fixtures/ directory.

use std::path::{Path, PathBuf};

use hydra_core::{Catalogue, Circuit, SCHEMA_VERSION, Severity};
use hydra_symbols::{
    Align, Primitive, block_outline, bounds, outline_issues, render_circuit, render_symbol, to_pdf,
    to_svg, wire_problems, wire_warnings,
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
        let wiring = [
            wire_problems(&circuit, &catalogue),
            wire_warnings(&circuit, &catalogue),
        ];
        assert_eq!(wiring, [Vec::<String>::new(), vec![]], "{name}");
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
                                  "X2": {"x": 10, "y": 45}, "X3": {"x": 100, "y": 30}}},
        "in_block": ["P", "X1"]}"#;
    let mut circuit = Circuit::from_json(json).unwrap();
    let (min, max) = block_outline(&circuit, &catalogue).unwrap();
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
    // X2 is drawn inside but isn't in the block; X3 is clear of it.
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
    assert_eq!(&with[..without.len()], &without[..]);
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
    circuit.layout.components.get_mut("X2").unwrap().y = 60.0 + 3.0;
    let issues = outline_issues(&circuit, &catalogue);
    assert_eq!(issues, [hydra_core::Issue::DrawnInsideBlock("X2".into())]);

    // A block of one port is a point, and draws nothing.
    circuit.in_block = ["P".to_string()].into();
    assert_eq!(
        block_outline(&circuit, &catalogue),
        Some(([0.0, 60.0], [0.0, 60.0]))
    );
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

fn motors() -> Circuit {
    let path = fixtures().join("circuits/demo-import-motors.json");
    Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn polylines(circuit: &Circuit) -> Vec<Vec<[f32; 2]>> {
    render_circuit(circuit, &catalogue())
        .primitives
        .into_iter()
        .filter_map(|p| match p {
            Primitive::Polyline { points, .. } => Some(points),
            _ => None,
        })
        .collect()
}

fn dot_at(circuit: &Circuit, at: [f32; 2]) -> bool {
    render_circuit(circuit, &catalogue())
        .primitives
        .iter()
        .any(|p| matches!(p, Primitive::Circle { center, filled: true, .. } if *center == at))
}

#[test]
fn drawn_wires_replace_the_automatic_ones() {
    let mut circuit = motors();
    let drawn = polylines(&circuit);
    for w in circuit.layout.wires.values().flatten() {
        assert!(drawn.contains(w), "{w:?} is drawn");
    }
    // FC1.Out to HM1.A: as drawn, not the automatic elbow from the net's first port.
    let auto = vec![[40.0, 52.0], [40.0, 52.0], [40.0, 23.0]];
    assert!(!drawn.contains(&auto));
    let first = |c: &Circuit| polylines(c).into_iter().filter(|w| w == &auto).count();
    circuit.layout.wires.remove("n-1");
    assert_eq!(
        first(&circuit),
        1,
        "a net without wires gets its elbow back"
    );
    assert!(polylines(&circuit).contains(&circuit.layout.wires["n-p"][0]));
}

#[test]
fn junction_dots_mark_where_three_lines_meet() {
    let mut c = motors();
    // A wire ending partway along another (a T) gets a dot; a bend inside one wire doesn't.
    assert!(dot_at(&c, [90.0, 106.0]));
    assert!(!dot_at(&c, [0.0, 106.0]));
    // n-p's trunk split at (40, 106), where FC1.In's branch meets it: three ends, one dot.
    let n_p = c.layout.wires.get_mut("n-p").unwrap();
    n_p[0] = vec![[0.0, 110.0], [0.0, 106.0], [40.0, 106.0]];
    n_p.push(vec![[40.0, 106.0], [255.0, 106.0], [255.0, 104.8]]);
    assert!(dot_at(&c, [40.0, 106.0]));
    let dots = |c: &Circuit| {
        (render_circuit(c, &catalogue()).primitives.iter())
            .filter(|p| {
                matches!(
                    p,
                    Primitive::Circle {
                        center: [40.0, 106.0],
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(dots(&c), 1);
    // Two wires meeting end to end, as a corner: no dot.
    let mut c = motors();
    let n_t = c.layout.wires.get_mut("n-t").unwrap();
    n_t[0] = vec![[20.0, 110.0], [20.0, 50.0]];
    n_t.push(vec![[20.0, 50.0], [20.0, 2.0], [230.0, 2.0], [230.0, 72.0]]);
    assert!(!dot_at(&c, [20.0, 50.0]));
    assert_eq!(wire_problems(&c, &catalogue()), Vec::<String>::new());
}

#[test]
fn drawn_wires_must_join_their_ports() {
    let catalogue = catalogue();
    let problems = |c: &Circuit| wire_problems(c, &catalogue);
    let mut c = motors();
    // FC1.Out's wire stops short of HM1.A.
    c.layout
        .wires
        .insert("n-1".into(), vec![vec![[40.0, 52.0], [40.0, 30.0]]]);
    assert_eq!(
        problems(&c),
        [
            "wire 1 of net n-1 ends at 40, 30, on no port or other wire of the net; end it on one \
             (the net's ports: FC1.Out at 40, 52; HM1.A at 40, 23)",
            "port HM1.A at 40, 23 is on net n-1, but none of the net's wires reaches it; draw one \
             to it"
        ]
    );
    // Within half a millimetre counts, and a port partway along a wire is reached.
    c.layout.wires.insert(
        "n-1".into(),
        vec![vec![[40.0, 52.4], [40.0, 10.0], [40.3, 10.0]]],
    );
    let p = problems(&c);
    assert!(
        p.len() == 1 && p[0].starts_with("wire 1 of net n-1 ends at 40.3, 10,"),
        "{p:?}"
    );
    // Wires that each end on a port but only cross the rest of their net.
    let mut c = motors();
    c.layout.wires.get_mut("n-t").unwrap()[4] = vec![[48.0, 63.0], [62.0, 63.0], [62.0, -5.0]];
    c.layout.wires.get_mut("n-t").unwrap()[1] = vec![[40.0, 7.0], [40.0, -5.0], [62.0, -5.0]];
    let p = problems(&c);
    assert_eq!(
        p,
        [
            "the wires of net n-t are drawn in 2 separate pieces (wires 1, 3, 4, 6, 7, 8; wires \
          2, 5); join them by ending a wire on another, as wires that only cross are not joined"
        ]
    );
}

#[test]
fn drawn_wires_must_keep_off_other_nets() {
    let catalogue = catalogue();
    let problems = |c: &Circuit| wire_problems(c, &catalogue);
    // Over HM1.A, on n-1, and over the end of n-1's wire.
    let mut c = motors();
    c.layout
        .wires
        .get_mut("n-t")
        .unwrap()
        .push(vec![[20.0, 23.0], [62.0, 23.0]]);
    assert_eq!(
        problems(&c),
        [
            "wire 9 of net n-t passes over port HM1.A, which is on net n-1; route it around",
            "wire 1 of net n-1 touches wire 9 of net n-t at 40, 23; a wire may cross another \
             net's wire, but never end, turn or run along it",
        ]
    );
    // Through RV1, over RV1.P, on n-p, and along n-p's branch to it.
    let mut c = motors();
    c.layout.wires.get_mut("n-t").unwrap()[7] = vec![[20.0, 100.0], [185.0, 100.0], [185.0, 2.0]];
    let p = problems(&c);
    assert!(
        p.contains(
            &"wire 8 of net n-t passes over port RV1.P, which is on net n-p; route it \
             around"
                .to_string()
        ),
        "{p:?}"
    );
    assert!(
        p.iter()
            .any(|p| p.starts_with("wire 5 of net n-p touches wire 8 of net n-t at 185, 88")),
        "{p:?}"
    );
    assert!(wire_warnings(&c, &catalogue).contains(
        &"a wire of net n-t runs through RV1; check its route against the drawing".to_string()
    ));
    // Crossing another net's wire at a point is fine: n-t's x = 20 crosses n-p's trunk.
    assert_eq!(problems(&motors()), Vec::<String>::new());
    // A port on no net counts too.
    let mut c = motors();
    c.nets.retain(|n| n.id != "n-1");
    c.layout.wires.remove("n-1");
    c.layout
        .wires
        .get_mut("n-t")
        .unwrap()
        .push(vec![[20.0, 23.0], [62.0, 23.0]]);
    let p = problems(&c);
    assert!(
        p.contains(
            &"wire 9 of net n-t passes over port HM1.A, which is on no net; route it \
             around"
                .to_string()
        ),
        "{p:?}"
    );
}

#[test]
fn wires_are_checked_only_when_they_can_be_quickly() {
    let catalogue = catalogue();
    let mut c = motors();
    // Malformed or too many: import_problems' to report, so nothing here, and no work done.
    c.layout
        .wires
        .insert("n-1".into(), vec![vec![[40.0, 52.0]]]);
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    c.layout
        .wires
        .insert("n-1".into(), vec![vec![[40.0, 52.0], [40.0, f32::NAN]]]);
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    let loose = vec![vec![[40.0, 30.0], [40.0, 31.0]]; hydra_core::MAX_WIRES_PER_NET + 1];
    c.layout.wires.insert("n-1".into(), loose.clone());
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    assert_eq!(wire_warnings(&c, &catalogue), Vec::<String>::new());
    // Within the caps they are checked, and a long list is cut short: 30 wires off by
    // themselves make 60 loose ends, 2 unreached ports and 1 net in pieces.
    let apart = (0..30).map(|k| vec![[300.0 + k as f32, 0.0], [300.0 + k as f32, 1.0]]);
    c.layout.wires.insert("n-1".into(), apart.collect());
    let p = wire_problems(&c, &catalogue);
    assert_eq!(p.len(), hydra_core::MAX_LISTED + 1);
    assert_eq!(
        p[hydra_core::MAX_LISTED],
        "and 43 more wire problems like these"
    );
    // A net with a part that can't be drawn is validation's to report.
    let mut c = motors();
    c.components.retain(|x| x.id != "HM1");
    c.layout
        .wires
        .insert("n-1".into(), vec![vec![[40.0, 52.0], [40.0, 30.0]]]);
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
}

#[test]
fn a_wire_through_a_part_is_a_warning() {
    let catalogue = catalogue();
    let mut c = motors();
    c.layout
        .wires
        .get_mut("n-1")
        .unwrap()
        .push(vec![[30.0, 60.0], [100.0, 60.0]]);
    // Through FC2 and through FC1, though the net has a port of FC1: not into that port.
    assert_eq!(
        wire_warnings(&c, &catalogue),
        [
            "a wire of net n-1 runs through FC1; check its route against the drawing",
            "a wire of net n-1 runs through FC2; check its route against the drawing",
        ]
    );
}
