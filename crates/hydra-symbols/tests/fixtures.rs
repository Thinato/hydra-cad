//! Symbol snapshots and golden circuits, from the repo's fixtures/ directory.

use std::path::{Path, PathBuf};

use hydra_core::{Catalogue, Circuit, SCHEMA_VERSION, Severity, Wire};
use hydra_symbols::{
    Align, Primitive, block_outline, bounds, outline_issues, render_circuit, render_parts,
    render_symbol, to_pdf, to_svg, wire_problems, wire_warnings,
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

#[test]
fn designators_are_clear_of_wires_frames_and_the_outline() {
    let catalogue = catalogue();
    for entry in std::fs::read_dir(fixtures().join("circuits")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let circuit = Circuit::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let drawn = render_parts(&circuit, &catalogue);
        let parts: Vec<&Primitive> = drawn.iter().flat_map(|(_, d)| &d.primitives).collect();
        let all = render_circuit(&circuit, &catalogue).primitives;
        let frame = |p: &&Primitive| {
            matches!(
                p,
                Primitive::Polyline {
                    closed: true,
                    dashed: true,
                    ..
                }
            )
        };
        // Wires and the block outline (all the circuit draws besides its parts), and status
        // frames (the parts' closed dashed outlines).
        let lines: Vec<(&[[f32; 2]], bool)> = all
            .iter()
            .filter(|p| !parts.contains(p))
            .chain(parts.iter().copied().filter(frame))
            .filter_map(|p| match p {
                Primitive::Polyline { points, closed, .. } => Some((points.as_slice(), *closed)),
                _ => None,
            })
            .collect();
        for c in circuit
            .components
            .iter()
            .filter(|c| !matches!(c.kind, hydra_core::ComponentKind::Port { .. }))
        {
            let (_, part) = drawn.iter().find(|(id, _)| *id == c.id).unwrap();
            let label = part
                .primitives
                .iter()
                .find(|p| matches!(p, Primitive::Text { text, .. } if *text == c.id))
                .unwrap_or_else(|| panic!("{}: no designator", c.id));
            let (lo, hi) = bounds(std::slice::from_ref(label)).unwrap();
            for &(points, closed) in &lines {
                // A closed outline runs back to its first point.
                let back = if closed { &points[..1] } else { &[] };
                let ring: Vec<_> = points.iter().chain(back).collect();
                for seg in ring.windows(2) {
                    assert!(
                        !crosses(*seg[0], *seg[1], lo, hi),
                        "{}: designator {} {lo:?}-{hi:?} touches {seg:?}",
                        path.display(),
                        c.id
                    );
                }
            }
        }
    }
}

#[test]
fn a_designator_moved_by_a_wire_counts_where_it_is_drawn() {
    let catalogue = catalogue();
    // The wire from Q to R runs down just left of X100, so its designator goes on the right.
    let json = r#"{"schema": 4, "name": "w", "components": [
        {"id": "Q", "kind": "port", "label": "Q"}, {"id": "R", "kind": "port", "label": "R"},
        {"id": "X100", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "Z1", "kind": "item", "symbol": {"kind": "orifice"}}],
        "nets": [{"id": "n", "ports": [{"component": "Q", "port": "port"}, {"component": "R", "port": "port"}]}],
        "layout": {"components": {"Q": {"x": 26, "y": 0}, "R": {"x": 26, "y": 60},
                                  "X100": {"x": 30, "y": 30}, "Z1": {"x": 46, "y": 30}}},
        "in_block": ["X100"]}"#;
    let circuit = Circuit::from_json(json).unwrap();
    let parts = render_parts(&circuit, &catalogue);
    let (_, x100) = parts.iter().find(|(id, _)| id == "X100").unwrap();
    let label = x100
        .primitives
        .iter()
        .find(|p| matches!(p, Primitive::Text { text, .. } if text == "X100"))
        .unwrap();
    let (lo, hi) = bounds(std::slice::from_ref(label)).unwrap();
    assert!(lo[0] > 30.0, "drawn on the right: {lo:?}");
    // What hit testing and the outline see is what render_circuit draws.
    assert!(
        render_circuit(&circuit, &catalogue)
            .primitives
            .contains(label)
    );
    let (min, max) = bounds_of(&block_outline(&circuit, &catalogue).unwrap());
    assert!(
        min[0] < lo[0] && min[1] < lo[1] && hi[0] < max[0] && hi[1] < max[1],
        "label {lo:?}-{hi:?} outside the outline {min:?}-{max:?}"
    );
    // Z1 is clear of X100's drawing without its designator, but not of the outline round it.
    assert_eq!(
        outline_issues(&circuit, &catalogue),
        [hydra_core::Issue::DrawnInsideBlock("Z1".into())]
    );
}

/// Whether segment `a`-`b` passes through the box `lo`-`hi` (Liang-Barsky clipping).
fn crosses(a: [f32; 2], b: [f32; 2], lo: [f32; 2], hi: [f32; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..2 {
        let d = b[k] - a[k];
        for (p, q) in [(-d, a[k] - lo[k]), (d, hi[k] - a[k])] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
    }
    t0 <= t1
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
fn pilot_and_drain_wires_are_dashed_and_plugged_ports_closed_with_a_bar() {
    let catalogue = catalogue();
    // One net: a working line X-Y, and a pilot line from Z branching off it.
    let json = r#"{"schema": 5, "name": "w", "components": [
        {"id": "X", "kind": "port", "label": "X"}, {"id": "Y", "kind": "port", "label": "Y", "plugged": true},
        {"id": "Z", "kind": "port", "label": "Z"}],
        "nets": [{"id": "n", "ports": [{"component": "X", "port": "port"}, {"component": "Y", "port": "port"}, {"component": "Z", "port": "port"}]}],
        "layout": {"components": {"X": {"x": 0, "y": 0}, "Y": {"x": 20, "y": 0}, "Z": {"x": 10, "y": 10}},
                   "wires": {"n": [[[0, 0], [20, 0]], {"kind": "pilot", "points": [[10, 10], [10, 0]]}]}}}"#;
    let mut circuit = Circuit::from_json(json).unwrap();
    assert_eq!(wire_problems(&circuit, &catalogue), Vec::<String>::new());
    let lines = |c: &Circuit| {
        let drawing = render_circuit(c, &catalogue);
        let lines: Vec<(Vec<[f32; 2]>, bool, bool)> = (drawing.primitives.into_iter())
            .filter_map(|p| match p {
                Primitive::Polyline {
                    points,
                    dashed,
                    filled,
                    ..
                } => Some((points, dashed, filled)),
                _ => None,
            })
            .collect();
        lines
    };
    let drawn = lines(&circuit);
    assert!(drawn.contains(&(vec![[0.0, 0.0], [20.0, 0.0]], false, false)));
    assert!(drawn.contains(&(vec![[10.0, 10.0], [10.0, 0.0]], true, false)));
    // Y is closed by a bar across the line ending there, upright here, and has no circle.
    let bar = |c: &Circuit| {
        let found: Vec<_> = lines(c)
            .into_iter()
            .filter(|(_, _, filled)| *filled)
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        let (lo, hi) = bounds_of(&found[0].0);
        // Relative to Y, to a hundredth of a millimetre.
        let r = |v: f32| (v * 100.0).round() / 100.0;
        ([r(lo[0] - 20.0), r(lo[1])], [r(hi[0] - 20.0), r(hi[1])])
    };
    assert_eq!(bar(&circuit), ([-0.35, -1.5], [0.35, 1.5]));
    let parts = render_parts(&circuit, &catalogue);
    let circles = |id: &str| {
        let (_, d) = parts.iter().find(|(p, _)| p == id).unwrap();
        (d.primitives.iter())
            .filter(|p| matches!(p, Primitive::Circle { .. }))
            .count()
    };
    assert_eq!((circles("X"), circles("Y")), (1, 0));
    // Untraced, the net is drawn with solid elbows: a dashed line has to be traced.
    circuit.layout.wires.clear();
    assert!(lines(&circuit).iter().all(|(_, dashed, _)| !dashed));
    assert_eq!(bar(&circuit), ([-0.35, -1.5], [0.35, 1.5]));
    // A plugged port with no line reaching it gets a level bar, as for an upright line.
    circuit.nets.clear();
    assert_eq!(bar(&circuit), ([-1.5, -0.35], [1.5, 0.35]));
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
    // Everything else is drawn the same; only port P's label moves outside the block.
    let not_p = |v: &[Primitive]| -> Vec<Primitive> {
        v.iter()
            .filter(|p| !matches!(p, Primitive::Text { text, .. } if text == "P"))
            .cloned()
            .collect()
    };
    assert_eq!(not_p(&with[..without.len()]), not_p(&without));
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
    // outside it in the middle of the L.
    let json = r#"{"schema": 4, "name": "l", "components": [
        {"id": "A", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "B", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "C", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "N", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "M", "kind": "item", "symbol": {"kind": "orifice"}}],
        "layout": {"components": {"A": {"x": 0, "y": 0}, "B": {"x": 0, "y": 40},
            "C": {"x": 40, "y": 40}, "N": {"x": 40, "y": 0}, "M": {"x": 20, "y": 20}}},
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
    // N in the notch is fine; M amid the block parts is drawn inside the block.
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
        place(20.0, 20.0),
        &catalogue
    ));

    // With nothing in the corner the block stays a rectangle.
    circuit.components.retain(|c| c.id != "N");
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [min, [max[0], min[1]], max, [min[0], max[1]]]
    );
}

/// Bounds of `id` drawn on its own.
fn alone(circuit: &Circuit, id: &str, catalogue: &Catalogue) -> ([f32; 2], [f32; 2]) {
    let mut c = circuit.clone();
    c.components.retain(|c| c.id == id);
    c.in_block.clear();
    bounds(&render_circuit(&c, catalogue).primitives).unwrap()
}

#[test]
fn a_notch_can_be_cut_at_every_corner() {
    let catalogue = catalogue();
    let at = [[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0]];
    // Corners clockwise from the top left; block parts on three, outside part N on the fourth.
    for k in 0..4 {
        let others: Vec<usize> = (0..4).filter(|&j| j != k).collect();
        let json = format!(
            r#"{{"schema": 4, "name": "l", "components": [
                {{"id": "A", "kind": "item", "symbol": {{"kind": "orifice"}}}},
                {{"id": "B", "kind": "item", "symbol": {{"kind": "orifice"}}}},
                {{"id": "C", "kind": "item", "symbol": {{"kind": "orifice"}}}},
                {{"id": "N", "kind": "item", "symbol": {{"kind": "orifice"}}}}],
                "layout": {{"components": {{"A": {{"x": {}, "y": {}}}, "B": {{"x": {}, "y": {}}},
                    "C": {{"x": {}, "y": {}}}, "N": {{"x": {}, "y": {}}}}}}},
                "in_block": ["A", "B", "C"]}}"#,
            at[others[0]][0],
            at[others[0]][1],
            at[others[1]][0],
            at[others[1]][1],
            at[others[2]][0],
            at[others[2]][1],
            at[k][0],
            at[k][1],
        );
        let circuit = Circuit::from_json(&json).unwrap();
        // The block's rectangle: room round its three parts.
        let (n0, n1) = alone(&circuit, "N", &catalogue);
        let (mut min, mut max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for id in ["A", "B", "C"] {
            let (lo, hi) = alone(&circuit, id, &catalogue);
            min = [min[0].min(lo[0] - 4.0), min[1].min(lo[1] - 4.0)];
            max = [max[0].max(hi[0] + 4.0), max[1].max(hi[1] + 4.0)];
        }
        let (tl, tr, br, bl) = (min, [max[0], min[1]], max, [min[0], max[1]]);
        let expected = match k {
            0 => {
                let i = [n1[0] + 4.0, n1[1] + 4.0];
                vec![[min[0], i[1]], i, [i[0], min[1]], tr, br, bl]
            }
            1 => {
                let i = [n0[0] - 4.0, n1[1] + 4.0];
                vec![tl, [i[0], min[1]], i, [max[0], i[1]], br, bl]
            }
            2 => {
                let i = [n0[0] - 4.0, n0[1] - 4.0];
                vec![tl, tr, [max[0], i[1]], i, [i[0], max[1]], bl]
            }
            _ => {
                let i = [n1[0] + 4.0, n0[1] - 4.0];
                vec![tl, tr, br, [i[0], max[1]], i, [min[0], i[1]]]
            }
        };
        assert_eq!(
            block_outline(&circuit, &catalogue).unwrap(),
            expected,
            "corner {k}"
        );
        assert!(
            outline_issues(&circuit, &catalogue).is_empty(),
            "corner {k}"
        );
        assert_eq!(hydra_symbols::notched_parts(&circuit, &catalogue), ["N"]);
    }
}

#[test]
fn notches_never_meet_and_surrounded_parts_stay_flagged() {
    let catalogue = catalogue();
    // Ports L, P, R are the block; N1 and N2 stand outside it on either side of P on the top
    // edge. Either notch, with room, would end exactly at P, and meet the other's there, leaving
    // the top edge a spike: a port on a cut's edge stops it, so neither has room on its P side.
    let json = r#"{"schema": 4, "name": "n", "components": [
        {"id": "L", "kind": "port", "label": "L"}, {"id": "P", "kind": "port", "label": "P"},
        {"id": "R", "kind": "port", "label": "R"},
        {"id": "N1", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "N2", "kind": "item", "symbol": {"kind": "orifice"}}],
        "layout": {"components": {"L": {"x": 0, "y": 40}, "P": {"x": 26.5, "y": 0},
            "R": {"x": 100, "y": 40}, "N1": {"x": 20, "y": 0},
            "N2": {"x": 36.0287523, "y": 0}}},
        "in_block": ["L", "P", "R"]}"#;
    let circuit = Circuit::from_json(json).unwrap();
    let (n1, n2) = (
        alone(&circuit, "N1", &catalogue),
        alone(&circuit, "N2", &catalogue),
    );
    assert_eq!(
        (n1.1[0] + 4.0, n2.0[0] - 4.0),
        (26.5, 26.5),
        "set up to meet at P"
    );
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            [0.0, n1.1[1] + 4.0],
            [n1.1[0], n1.1[1] + 4.0],
            [n1.1[0], 0.0],
            [n2.0[0], 0.0],
            [n2.0[0], n2.1[1] + 4.0],
            [100.0, n2.1[1] + 4.0],
            [100.0, 40.0],
            [0.0, 40.0],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());

    // Block parts at opposite corners and an outside part between them: a mistake, not a notch.
    let json = r#"{"schema": 4, "name": "d", "components": [
        {"id": "A", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "B", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "O", "kind": "item", "symbol": {"kind": "orifice"}}],
        "layout": {"components": {"A": {"x": 0, "y": 0}, "B": {"x": 50, "y": 70},
            "O": {"x": 25, "y": 35}}},
        "in_block": ["A", "B"]}"#;
    let circuit = Circuit::from_json(json).unwrap();
    assert_eq!(block_outline(&circuit, &catalogue).unwrap().len(), 4);
    assert_eq!(
        outline_issues(&circuit, &catalogue),
        [hydra_core::Issue::DrawnInsideBlock("O".into())]
    );
    assert!(hydra_symbols::notched_parts(&circuit, &catalogue).is_empty());
}

/// A drawing's bounds.
type Extent = ([f32; 2], [f32; 2]);

/// Orifices `ids` placed at `at`, with `in_block` the block, and the bounds of each drawn alone.
fn orifices(ids: &[(&str, f32, f32)], in_block: &[&str]) -> (Circuit, Vec<Extent>) {
    let components: Vec<String> = ids
        .iter()
        .map(|(id, _, _)| {
            format!(r#"{{"id": "{id}", "kind": "item", "symbol": {{"kind": "orifice"}}}}"#)
        })
        .collect();
    let layout: Vec<String> = ids
        .iter()
        .map(|(id, x, y)| format!(r#""{id}": {{"x": {x}, "y": {y}}}"#))
        .collect();
    let json = format!(
        r#"{{"schema": 4, "name": "o", "components": [{}], "layout": {{"components": {{{}}}}},
            "in_block": {in_block:?}}}"#,
        components.join(","),
        layout.join(",")
    );
    let circuit = Circuit::from_json(&json).unwrap();
    let catalogue = catalogue();
    let bounds = ids
        .iter()
        .map(|(id, _, _)| alone(&circuit, id, &catalogue))
        .collect();
    (circuit, bounds)
}

/// Whether every dash of the drawn outline lies on an edge of `block_outline`.
fn dashes_on_outline(circuit: &Circuit, catalogue: &Catalogue) -> bool {
    let corners = block_outline(circuit, catalogue).unwrap();
    let on_edge = |p: [f32; 2]| {
        (0..corners.len()).any(|k| {
            let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
            let within = |v: f32, a: f32, b: f32| a.min(b) - 1e-3 <= v && v <= a.max(b) + 1e-3;
            within(p[0], a[0], b[0]) && within(p[1], a[1], b[1])
        })
    };
    outline_lines(circuit, catalogue)
        .iter()
        .flatten()
        .all(|&p| on_edge(p))
}

#[test]
fn an_outside_part_at_an_edge_between_block_parts_gets_a_u_cut() {
    let catalogue = catalogue();
    // Block parts A, B, C make an L with N in its notch; M stands outside the block on its left
    // edge, between A and B.
    let (mut circuit, b) = orifices(
        &[
            ("A", 0.0, 0.0),
            ("B", 0.0, 40.0),
            ("C", 40.0, 40.0),
            ("N", 40.0, 0.0),
            ("M", 0.0, 20.0),
        ],
        &["A", "B", "C"],
    );
    let [a, bb, c, n, m] = b[..] else { panic!() };
    let (min, max) = ([a.0[0] - 4.0, a.0[1] - 4.0], [c.1[0] + 4.0, c.1[1] + 4.0]);
    let notch = [n.0[0] - 4.0, n.1[1] + 4.0];
    // The outline goes in round M and back out, with room beside M but none above or below it,
    // where A and B are too close.
    assert!(m.0[1] - 4.0 < a.1[1] + 4.0 && bb.0[1] - 4.0 < m.1[1] + 4.0);
    let (top, bottom, inner) = (m.0[1], m.1[1], m.1[0] + 4.0);
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            min,
            [notch[0], min[1]],
            notch,
            [max[0], notch[1]],
            max,
            [min[0], max[1]],
            [min[0], bottom],
            [inner, bottom],
            [inner, top],
            [min[0], top],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());
    assert_eq!(
        hydra_symbols::notched_parts(&circuit, &catalogue),
        ["N", "M"]
    );
    assert!(dashes_on_outline(&circuit, &catalogue));

    // A block port on the edge where the cut would go keeps it whole: M is flagged instead.
    circuit.components.push(hydra_core::Component {
        id: "P".into(),
        kind: hydra_core::ComponentKind::Port { label: "P".into() },
    });
    circuit.layout.components.insert(
        "P".into(),
        hydra_core::Placement {
            x: min[0],
            y: 20.0,
            ..Default::default()
        },
    );
    circuit.in_block.insert("P".into());
    assert_eq!(block_outline(&circuit, &catalogue).unwrap().len(), 6);
    assert_eq!(
        outline_issues(&circuit, &catalogue),
        [hydra_core::Issue::DrawnInsideBlock("M".into())]
    );
    assert_eq!(hydra_symbols::notched_parts(&circuit, &catalogue), ["N"]);

    // With room between M and B, M looks left out of the block by mistake, so it is flagged.
    circuit.components.retain(|c| c.id != "P");
    circuit.in_block.remove("P");
    circuit.layout.components.get_mut("B").unwrap().y = 60.0;
    assert_eq!(
        outline_issues(&circuit, &catalogue),
        [hydra_core::Issue::DrawnInsideBlock("M".into())]
    );
}

#[test]
fn u_cuts_go_in_from_the_bottom_and_right_edges() {
    let catalogue = catalogue();
    // V1, left out of the motors block, stands at its bottom edge snug between RV1 and M1.
    let mut circuit = motors();
    circuit.in_block = ["P", "T", "FC1", "FC2", "FC3", "RV1", "M1"]
        .map(String::from)
        .into();
    let parts = render_parts(&circuit, &catalogue);
    let at = |id: &str| bounds(&parts.iter().find(|(p, _)| p == id).unwrap().1.primitives).unwrap();
    let (v, m1) = (at("V1"), at("M1"));
    let corners = block_outline(&circuit, &catalogue).unwrap();
    let (min, max) = bounds_of(&corners);
    // Room left of V1 and above it, none on its right, where M1 is too close.
    assert!(m1.0[0] - 4.0 < v.1[0] + 4.0);
    let (left, top, right) = (v.0[0] - 4.0, v.0[1] - 4.0, v.1[0]);
    assert_eq!(
        corners,
        [
            min,
            [max[0], min[1]],
            max,
            [right, max[1]],
            [right, top],
            [left, top],
            [left, max[1]],
            [min[0], max[1]],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());
    assert_eq!(hydra_symbols::notched_parts(&circuit, &catalogue), ["V1"]);

    // M stands at the right edge of a square of block parts, between C and D.
    let (circuit, b) = orifices(
        &[
            ("A", 0.0, 0.0),
            ("B", 0.0, 40.0),
            ("C", 40.0, 0.0),
            ("D", 40.0, 40.0),
            ("M", 40.0, 20.0),
        ],
        &["A", "B", "C", "D"],
    );
    let [a, bb, c, d, m] = b[..] else { panic!() };
    let (min, max) = ([a.0[0] - 4.0, a.0[1] - 4.0], [d.1[0] + 4.0, bb.1[1] + 4.0]);
    // Room beside M, none above or below it, where C and D are too close.
    assert!(m.0[1] - 4.0 < c.1[1] + 4.0 && d.0[1] - 4.0 < m.1[1] + 4.0);
    let inner = m.0[0] - 4.0;
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            min,
            [max[0], min[1]],
            [max[0], m.0[1]],
            [inner, m.0[1]],
            [inner, m.1[1]],
            [max[0], m.1[1]],
            max,
            [min[0], max[1]],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());
    assert!(dashes_on_outline(&circuit, &catalogue));
}

#[test]
fn notches_at_opposite_corners_never_meet_at_a_point() {
    let catalogue = catalogue();
    // Block parts A and B in the other corners, spaced so N1's and N2's notches, each with room,
    // would meet at one point and pinch the block in two.
    // N2 at a round spot, and N1 nudged by the last bit until its notch meets N2's exactly.
    let n2 = orifices(&[("N2", 20.0, 30.0)], &[]).1[0].0;
    let n1_hi = |d: f32| orifices(&[("N1", d, d)], &[]).1[0].1;
    let at = |a: usize| {
        let mut d = n2[a] - 8.0 - n1_hi(0.0)[a] - 1e-5;
        while n1_hi(d)[a] + 4.0 < n2[a] - 4.0 {
            d = d.next_up();
        }
        d
    };
    let (x, y) = (at(0), at(1));
    let (circuit, b) = orifices(
        &[
            ("N1", x, y),
            ("A", 20.0, y),
            ("B", x, 30.0),
            ("N2", 20.0, 30.0),
        ],
        &["A", "B"],
    );
    let [n1, a, bb, n2] = b[..] else { panic!() };
    let p = [n1.1[0] + 4.0, n1.1[1] + 4.0];
    assert_eq!(p, [n2.0[0] - 4.0, n2.0[1] - 4.0], "set up to meet");
    let (min, max) = ([bb.0[0] - 4.0, a.0[1] - 4.0], [a.1[0] + 4.0, bb.1[1] + 4.0]);
    // N2's notch keeps clear of N1's: no room above it.
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            [min[0], p[1]],
            p,
            [p[0], min[1]],
            [max[0], min[1]],
            [max[0], n2.0[1]],
            [p[0], n2.0[1]],
            [p[0], max[1]],
            [min[0], max[1]],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());
}

#[test]
fn outside_parts_stacked_in_a_corner_get_a_stepped_outline() {
    let catalogue = catalogue();
    // A three by three grid: N1 and N2 along the top and N3 below N1 stand outside the block in
    // its top-left corner. One notch round all three would take in block part D.
    let (circuit, b) = orifices(
        &[
            ("N1", 0.0, 0.0),
            ("N2", 20.0, 0.0),
            ("N3", 0.0, 20.0),
            ("A", 40.0, 0.0),
            ("D", 20.0, 20.0),
            ("B", 40.0, 20.0),
            ("C", 0.0, 40.0),
            ("E", 20.0, 40.0),
            ("F", 40.0, 40.0),
        ],
        &["A", "B", "C", "D", "E", "F"],
    );
    let [_, n2, n3, a, d, _, c, _, f] = b[..] else {
        panic!()
    };
    let (min, max) = ([c.0[0] - 4.0, a.0[1] - 4.0], [f.1[0] + 4.0, f.1[1] + 4.0]);
    // Each step has room beside its part, but none below it, where D and C are too close.
    assert!(n2.1[1] + 4.0 > d.0[1] - 4.0 && n3.1[1] + 4.0 > c.0[1] - 4.0);
    let (wide, deep) = ([n2.1[0] + 4.0, n2.1[1]], [n3.1[0] + 4.0, n3.1[1]]);
    assert_eq!(
        block_outline(&circuit, &catalogue).unwrap(),
        [
            [min[0], deep[1]],
            deep,
            [deep[0], wide[1]],
            wide,
            [wide[0], min[1]],
            [max[0], min[1]],
            max,
            [min[0], max[1]],
        ]
    );
    assert!(outline_issues(&circuit, &catalogue).is_empty());
    assert_eq!(
        hydra_symbols::notched_parts(&circuit, &catalogue),
        ["N1", "N2", "N3"]
    );
    assert!(dashes_on_outline(&circuit, &catalogue));
}

#[test]
fn an_outside_part_off_the_grid_still_gets_its_notch() {
    let catalogue = catalogue();
    // Block valves in an L, and a motor in its notch a few millimetres off their grid: the
    // block parts' centres lie past its middle on every side, but not past its edges.
    let mut circuit = motors();
    circuit
        .components
        .retain(|c| ["FC1", "RV1", "V1", "HM1"].contains(&c.id.as_str()));
    circuit.nets.clear();
    circuit.layout.wires.clear();
    circuit.in_block = ["FC1", "RV1", "V1"].map(String::from).into();
    for (id, x, y) in [
        ("FC1", 40.0, 60.0),
        ("RV1", 40.0, 110.0),
        ("V1", 100.0, 110.0),
        ("HM1", 90.0, 64.0),
    ] {
        let p = circuit.layout.components.get_mut(id).unwrap();
        (p.x, p.y) = (x, y);
    }
    assert!(outline_issues(&circuit, &catalogue).is_empty());
    assert_eq!(hydra_symbols::notched_parts(&circuit, &catalogue), ["HM1"]);
    assert_eq!(block_outline(&circuit, &catalogue).unwrap().len(), 6);
}

#[test]
fn block_port_labels_sit_outside_and_the_outline_breaks_round_ports() {
    let catalogue = catalogue();
    let json = r#"{"schema": 4, "name": "p", "components": [
        {"id": "X", "kind": "item", "symbol": {"kind": "orifice"}},
        {"id": "T", "kind": "port", "label": "TOP"}, {"id": "B", "kind": "port", "label": "BOTTOM"},
        {"id": "L", "kind": "port", "label": "LEFT"}, {"id": "R", "kind": "port", "label": "RIGHT"},
        {"id": "C", "kind": "port", "label": "C"}],
        "layout": {"components": {"X": {"x": 0, "y": 0}, "T": {"x": 0, "y": -30},
            "B": {"x": 5, "y": 30}, "L": {"x": -30, "y": 3}, "R": {"x": 30, "y": -3},
            "C": {"x": 29.5, "y": 30}}},
        "in_block": ["X", "T", "B", "L", "R", "C"]}"#;
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
        // Hit testing sees the label where it is drawn.
        assert!(
            render_parts(&circuit, &catalogue)
                .iter()
                .any(|(_, d)| d.primitives.contains(text)),
            "{label}"
        );
    }
    // No dash runs into a port's circle, not even on the next edge round a corner (C).
    let lines = outline_lines(&circuit, &catalogue);
    for port in [
        [0.0, -30.0],
        [5.0, 30.0],
        [-30.0, 3.0],
        [30.0, -3.0],
        [29.5, 30.0],
    ] {
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
        assert!(drawn.contains(&w.points), "{w:?} is drawn");
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
    assert!(polylines(&circuit).contains(&circuit.layout.wires["n-p"][0].points));
}

#[test]
fn junction_dots_mark_where_three_lines_meet() {
    let mut c = motors();
    // A wire ending partway along another (a T) gets a dot; a bend inside one wire doesn't.
    assert!(dot_at(&c, [90.0, 106.0]));
    assert!(!dot_at(&c, [0.0, 106.0]));
    // n-p's trunk split at (40, 106), where FC1.In's branch meets it: three ends, one dot.
    let n_p = c.layout.wires.get_mut("n-p").unwrap();
    n_p[0] = vec![[0.0, 110.0], [0.0, 106.0], [40.0, 106.0]].into();
    n_p.push(vec![[40.0, 106.0], [255.0, 106.0], [255.0, 104.8]].into());
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
    n_t[0] = vec![[20.0, 110.0], [20.0, 50.0]].into();
    n_t.push(vec![[20.0, 50.0], [20.0, 2.0], [230.0, 2.0], [230.0, 72.0]].into());
    assert!(!dot_at(&c, [20.0, 50.0]));
    assert_eq!(wire_problems(&c, &catalogue()), Vec::<String>::new());
    // At a port, the port is one of the lines: M1.M, at the end of n-p's trunk.
    let mut c = motors();
    let m = [255.0, 104.8];
    assert!(!dot_at(&c, m), "one wire ending at M1.M");
    c.layout.wires.get_mut("n-p").unwrap()[0] = vec![
        [0.0, 110.0],
        [0.0, 106.0],
        [255.0, 106.0],
        [255.0, 100.0],
        [265.0, 100.0],
    ]
    .into();
    assert!(dot_at(&c, m), "a wire passing over M1.M");
    let n_p = c.layout.wires.get_mut("n-p").unwrap();
    n_p[0] = vec![[0.0, 110.0], [0.0, 106.0], [255.0, 106.0], [255.0, 104.8]].into();
    n_p.push(vec![[255.0, 104.8], [255.0, 100.0], [265.0, 100.0]].into());
    assert!(dot_at(&c, m), "two wire ends meeting at M1.M");
}

#[test]
fn drawn_wires_must_join_their_ports() {
    let catalogue = catalogue();
    let problems = |c: &Circuit| wire_problems(c, &catalogue);
    let mut c = motors();
    // FC1.Out's wire stops short of HM1.A.
    c.layout
        .wires
        .insert("n-1".into(), vec![vec![[40.0, 52.0], [40.0, 30.0]].into()]);
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
        vec![vec![[40.0, 52.4], [40.0, 10.0], [40.3, 10.0]].into()],
    );
    let p = problems(&c);
    assert!(
        p.len() == 1 && p[0].starts_with("wire 1 of net n-1 ends at 40.3, 10,"),
        "{p:?}"
    );
    // Wires that each end on a port but only cross the rest of their net.
    let mut c = motors();
    c.layout.wires.get_mut("n-t").unwrap()[4] =
        vec![[48.0, 63.0], [62.0, 63.0], [62.0, -5.0]].into();
    c.layout.wires.get_mut("n-t").unwrap()[1] =
        vec![[40.0, 7.0], [40.0, -5.0], [62.0, -5.0]].into();
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
        .push(vec![[20.0, 23.0], [62.0, 23.0]].into());
    assert_eq!(
        problems(&c),
        [
            "wire 9 of net n-t passes over port HM1.A, which is on net n-1; route it around",
            "wire 9 of net n-t and wire 1 of net n-1 touch at 40, 23; a wire may cross another \
             net's wire, but never end, turn or run along it, so move whichever differs from the \
             drawing",
        ]
    );
    // Through RV1, over RV1.P, on n-p, and along n-p's branch to it.
    let mut c = motors();
    c.layout.wires.get_mut("n-t").unwrap()[7] =
        vec![[20.0, 100.0], [185.0, 100.0], [185.0, 2.0]].into();
    // The touch is reported once, not from both wires.
    assert_eq!(
        problems(&c),
        [
            "wire 5 of net n-p and wire 8 of net n-t touch at 185, 88; a wire may cross another \
             net's wire, but never end, turn or run along it, so move whichever differs from the \
             drawing",
            "wire 8 of net n-t passes over port RV1.P, which is on net n-p; route it around",
        ]
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
        .push(vec![[20.0, 23.0], [62.0, 23.0]].into());
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
        .insert("n-1".into(), vec![vec![[40.0, 52.0]].into()]);
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    c.layout.wires.insert(
        "n-1".into(),
        vec![vec![[40.0, 52.0], [40.0, f32::NAN]].into()],
    );
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    let loose = vec![vec![[40.0, 30.0], [40.0, 31.0]]; hydra_core::MAX_WIRES_PER_NET + 1];
    let wires = loose.iter().cloned().map(Wire::from).collect();
    c.layout.wires.insert("n-1".into(), wires);
    assert_eq!(wire_problems(&c, &catalogue), Vec::<String>::new());
    assert_eq!(wire_warnings(&c, &catalogue), Vec::<String>::new());
    // Such wires are still drawn, without the junction dots that would take that work.
    assert!(polylines(&c).contains(&loose[0]));
    assert!(dot_at(&motors(), [90.0, 106.0]) && !dot_at(&c, [90.0, 106.0]));
    // Within the caps they are checked, and a long list is cut short: 30 wires off by
    // themselves make 60 loose ends, 2 unreached ports and 1 net in pieces.
    let apart = (0..30).map(|k| vec![[300.0 + k as f32, 0.0], [300.0 + k as f32, 1.0]].into());
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
        .insert("n-1".into(), vec![vec![[40.0, 52.0], [40.0, 30.0]].into()]);
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
        .push(vec![[30.0, 60.0], [100.0, 60.0]].into());
    // Through FC2 and through FC1, though the net has a port of FC1: not into that port.
    assert_eq!(
        wire_warnings(&c, &catalogue),
        [
            "a wire of net n-1 runs through FC1; check its route against the drawing",
            "a wire of net n-1 runs through FC2; check its route against the drawing",
        ]
    );
}
