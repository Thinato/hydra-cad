use hydra_core::*;
use serde_json::json;

/// A 4/3 closed-centre valve with solenoids and centring springs.
fn spool_4_3() -> SymbolDef {
    serde_json::from_value(json!({
        "kind": "directional_valve",
        "ports": [
            {"name": "A", "side": "top"}, {"name": "B", "side": "top"},
            {"name": "P", "side": "bottom"}, {"name": "T", "side": "bottom"}
        ],
        "positions": [
            {"flows": [["P", "A"], ["B", "T"]]},
            {"blocked": ["P", "T", "A", "B"]},
            {"flows": [["P", "B"], ["A", "T"]]}
        ],
        "normal_position": 1,
        "left": ["solenoid", "spring"],
        "right": ["solenoid", "spring"]
    }))
    .unwrap()
}

fn catalogue() -> Catalogue {
    Catalogue {
        manufacturers: vec![Manufacturer {
            key: "argo".into(),
            name: "Argo-Hytos".into(),
        }],
        symbol_defs: vec![
            SymbolDefRecord {
                key: "sym-4-3".into(),
                version: 1,
                definition: spool_4_3(),
            },
            SymbolDefRecord {
                key: "sym-check".into(),
                version: 1,
                definition: SymbolDef::Check { spring: true },
            },
        ],
        parts: vec![Part {
            part_number: "TEST-PART-1".into(),
            valve_type: CatalogueRef::new("rpe3-z11", 1),
            options: json!({"voltage": "24 V DC"}),
        }],
        valve_types: vec![
            ValveType {
                key: "rpe3-z11".into(),
                version: 1,
                manufacturer: "argo".into(),
                series: "RPE3-06".into(),
                code: "Z11".into(),
                symbol: Some(CatalogueRef::new("sym-4-3", 1)),
                mounting: Some("ISO 4401-03".into()),
                params: serde_json::Value::Null,
            },
            ValveType {
                key: "check".into(),
                version: 1,
                manufacturer: "argo".into(),
                series: "VJ3".into(),
                code: "10".into(),
                symbol: Some(CatalogueRef::new("sym-check", 1)),
                mounting: None,
                params: serde_json::Value::Null,
            },
        ],
        blocks: vec![],
    }
}

fn pr(component: &str, port: &str) -> PortRef {
    PortRef {
        component: component.into(),
        port: port.into(),
    }
}

fn net(id: &str, ports: &[(&str, &str)]) -> Net {
    Net {
        id: id.into(),
        name: None,
        ports: ports.iter().map(|(c, p)| pr(c, p)).collect(),
    }
}

fn valve(id: &str, valve_type: &str) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::valve(valve_type.to_string().try_into().unwrap()),
    }
}

fn port(id: &str) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Port { label: id.into() },
    }
}

/// Fully connected: P and T feed V1, V1's A goes through check K1 to A1, B to B1.
fn circuit() -> Circuit {
    Circuit {
        schema: SCHEMA_VERSION,
        assembly: None,
        in_block: Default::default(),
        name: "test".into(),
        components: vec![
            valve("V1", "rpe3-z11@1"),
            valve("K1", "check@1"),
            port("P"),
            port("T"),
            port("A1"),
            port("B1"),
        ],
        nets: vec![
            net("n-p", &[("P", PORT_PIN), ("V1", "P")]),
            net("n-t", &[("T", PORT_PIN), ("V1", "T")]),
            net("n-a", &[("V1", "A"), ("K1", "A")]),
            net("n-a1", &[("K1", "B"), ("A1", PORT_PIN)]),
            net("n-b", &[("V1", "B"), ("B1", PORT_PIN)]),
        ],
        layout: Layout::default(),
        bom_extra: vec![],
        import: None,
    }
}

#[test]
fn valid_circuit_has_no_issues() {
    assert_eq!(circuit().validate(&catalogue()), vec![]);
    assert_eq!(catalogue().problems(), Vec::<String>::new());
}

#[test]
fn json_round_trip_is_lossless() {
    let mut c = circuit();
    c.layout.components.insert(
        "V1".into(),
        Placement {
            x: 10.0,
            y: 20.5,
            rotation: Rotation::R270,
            mirror: true,
        },
    );
    c.layout
        .wires
        .insert("n-p".into(), vec![vec![[0.0, 0.0], [5.0, 0.0]]]);
    c.nets[0].name = Some("pressure".into());
    let json = c.to_json();
    assert_eq!(Circuit::from_json(&json).unwrap(), c);

    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema"], SCHEMA_VERSION);
    assert_eq!(value["components"][0]["valve_type"], "rpe3-z11@1");
    assert_eq!(value["layout"]["components"]["V1"]["rotation"], 270);
    assert_eq!(value["layout"]["components"]["V1"]["mirror"], true);
    // Unmirrored parts don't write the flag, and older documents read as unmirrored.
    assert!(json.matches("\"mirror\"").count() == 1, "{json}");
}

#[test]
fn catalogue_round_trip_is_lossless() {
    let json = serde_json::to_string(&catalogue()).unwrap();
    assert_eq!(
        serde_json::from_str::<Catalogue>(&json).unwrap(),
        catalogue()
    );
}

#[test]
fn unknown_or_missing_schema_is_an_error() {
    for v in [0, SCHEMA_VERSION + 1] {
        let json = format!(r#"{{"schema": {v}, "name": "x", "components": []}}"#);
        assert!(matches!(
            Circuit::from_json(&json),
            Err(LoadError::UnsupportedSchema(n)) if n == u64::from(v)
        ));
    }
    assert!(matches!(
        Circuit::from_json(r#"{"name": "x", "components": []}"#),
        Err(LoadError::MissingSchema)
    ));
    assert!(matches!(
        Circuit::from_json("not json"),
        Err(LoadError::Json(_))
    ));
}

#[test]
fn bad_rotation_is_rejected() {
    let json = r#"{"schema": 1, "name": "x", "components": [],
        "layout": {"components": {"V1": {"x": 0, "y": 0, "rotation": 45}}}}"#;
    assert!(Circuit::from_json(json).is_err());
}

#[test]
fn net_with_unknown_component() {
    let mut c = circuit();
    c.nets[0].ports.push(pr("V9", "P"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::UnknownComponent {
            net: "n-p".into(),
            component: "V9".into()
        }]
    );
}

#[test]
fn net_with_unknown_port() {
    let mut c = circuit();
    c.nets[0].ports.push(pr("K1", "X"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::UnknownPort {
            net: "n-p".into(),
            port: pr("K1", "X")
        }]
    );
}

#[test]
fn port_on_two_nets() {
    let mut c = circuit();
    c.nets[1].ports.push(pr("V1", "P"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::PortOnTwoNets {
            port: pr("V1", "P"),
            nets: ["n-p".into(), "n-t".into()]
        }]
    );
}

#[test]
fn unknown_valve_type() {
    let mut c = circuit();
    c.components.push(valve("V2", "rpe3-z11@2"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::UnknownValveType {
            component: "V2".into(),
            valve_type: CatalogueRef::new("rpe3-z11", 2)
        }]
    );
}

#[test]
fn duplicate_component() {
    let mut c = circuit();
    c.components.push(port("P"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::DuplicateComponent("P".into())]
    );
}

#[test]
fn duplicate_net_id_but_not_a_repeated_port() {
    let mut c = circuit();
    c.nets[0].ports.push(pr("V1", "P"));
    assert_eq!(c.validate(&catalogue()), vec![]);
    c.nets.push(net("n-p", &[]));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::DuplicateNet("n-p".into())]
    );
}

#[test]
fn unconnected_port_is_a_warning() {
    let mut c = circuit();
    c.nets.pop();
    let issues = c.validate(&catalogue());
    assert_eq!(
        issues,
        vec![
            Issue::Unconnected(pr("V1", "B")),
            Issue::Unconnected(pr("B1", PORT_PIN))
        ]
    );
    assert!(issues.iter().all(|i| i.severity() == Severity::Warning));
    assert_eq!(issues[0].to_string(), "port V1.B is not connected");
}

#[test]
fn malformed_symbols_are_rejected() {
    let SymbolDef::DirectionalValve(good) = spool_4_3() else {
        unreachable!()
    };
    assert!(spool_4_3().problems().is_empty());

    let mut no_positions = good.clone();
    no_positions.positions.clear();
    assert_eq!(
        SymbolDef::DirectionalValve(no_positions).problems(),
        ["a directional valve needs at least one spool position"]
    );

    let mut duplicate_port = good.clone();
    duplicate_port.ports.push(Port {
        name: "A".into(),
        side: Side::Bottom,
    });
    assert_eq!(
        SymbolDef::DirectionalValve(duplicate_port).problems(),
        ["port A is defined twice"]
    );

    let mut unknown_port = good.clone();
    unknown_port.positions[0]
        .flows
        .push(["P".into(), "X".into()]);
    assert_eq!(
        SymbolDef::DirectionalValve(unknown_port).problems(),
        ["position 1 refers to port X, which the valve does not have"]
    );

    let mut bad_normal = good;
    bad_normal.normal_position = 3;
    assert_eq!(
        SymbolDef::DirectionalValve(bad_normal).problems(),
        ["normal position 3 is out of range, the valve has 3 positions"]
    );
}

#[test]
fn catalogue_problems_name_unknown_rows() {
    let mut cat = catalogue();
    cat.valve_types[0].manufacturer = "bosch".into();
    cat.valve_types[1].symbol = Some(CatalogueRef::new("sym-check", 2));
    cat.valve_types.push(cat.valve_types[0].clone());
    assert_eq!(
        cat.problems(),
        [
            "valve type rpe3-z11@1: unknown manufacturer bosch",
            "valve type check@1: unknown symbol sym-check@2",
            "valve type rpe3-z11@1 is defined twice",
            "valve type rpe3-z11@1: unknown manufacturer bosch"
        ]
    );
}

#[test]
fn catalogue_refs_parse_and_print() {
    let r: CatalogueRef = "vt-rpe3-06-z11@3".to_string().try_into().unwrap();
    assert_eq!(r, CatalogueRef::new("vt-rpe3-06-z11", 3));
    assert_eq!(r.to_string(), "vt-rpe3-06-z11@3");
    // The key may itself contain '@'; the version is after the last one.
    let r: CatalogueRef = "a@b@2".to_string().try_into().unwrap();
    assert_eq!(r, CatalogueRef::new("a@b", 2));
    for bad in ["no-version", "@1", "key@0", "key@x", "key@"] {
        assert!(CatalogueRef::try_from(bad.to_string()).is_err(), "{bad}");
    }
}

#[test]
fn draft_valve_type_without_symbol() {
    let mut cat = catalogue();
    cat.valve_types[1].symbol = None;
    assert!(cat.problems().is_empty());
    assert_eq!(
        circuit().validate(&cat)[0],
        Issue::MissingSymbol {
            component: "K1".into(),
            valve_type: CatalogueRef::new("check", 1)
        }
    );
}

#[test]
fn keys_parts_and_duplicates_are_checked() {
    let mut cat = catalogue();
    assert_eq!(cat.parts_of(&CatalogueRef::new("rpe3-z11", 1)).count(), 1);
    cat.manufacturers.push(cat.manufacturers[0].clone());
    cat.symbol_defs[1].key = " sym-check".into();
    cat.valve_types[1].symbol = None;
    cat.parts.push(cat.parts[0].clone());
    cat.parts.push(Part {
        part_number: "X1".into(),
        valve_type: CatalogueRef::new("rpe3-z11", 9),
        options: serde_json::Value::Null,
    });
    assert_eq!(
        cat.problems(),
        [
            r#"symbol key " sym-check" has spaces at the start or end"#,
            "manufacturer argo is defined twice",
            "part TEST-PART-1 of rpe3-z11@1 is defined twice",
            "part X1: unknown valve type rpe3-z11@9",
        ]
    );
    assert_eq!(key_problem(""), Some("is empty"));
    assert_eq!(key_problem("a@b"), Some("contains '@'"));
    assert_eq!(key_problem("vt-rpe3-06-z11"), None);
}

#[test]
fn version_1_documents_load_and_save_as_the_current_version() {
    let c = Circuit::from_json(r#"{"schema": 1, "name": "old", "components": []}"#).unwrap();
    assert_eq!(c.schema, SCHEMA_VERSION);
    assert_eq!(c.assembly, None);
}

fn face(name: &str, pattern: &str, origin: [f64; 3], normal: [f64; 3]) -> MountFace {
    MountFace {
        name: name.into(),
        pattern: pattern.into(),
        origin,
        normal,
        x_axis: [1.0, 0.0, 0.0],
    }
}

/// Top faces F1 and F2 take CETOP 3 valves; front face F3 takes CETOP 5.
fn block() -> Block {
    Block {
        key: "blk".into(),
        version: 1,
        name: "Test block".into(),
        mount_faces: vec![
            face("F1", "ISO 4401-03", [45.0, 10.0, 70.0], [0.0, 0.0, 1.0]),
            face("F2", "ISO 4401-03", [45.0, 62.0, 70.0], [0.0, 0.0, 1.0]),
            face("F3", "ISO 4401-05", [30.0, 0.0, 15.0], [0.0, -1.0, 0.0]),
        ],
    }
}

fn on_block(mounts: &[(&str, &str)]) -> (Circuit, Catalogue) {
    let mut cat = catalogue();
    cat.blocks.push(block());
    let mut c = circuit();
    c.components.push(valve("V2", "rpe3-z11@1"));
    c.nets[0].ports.push(pr("V2", "P"));
    c.nets[1].ports.push(pr("V2", "T"));
    c.nets[2].ports.push(pr("V2", "A"));
    c.nets[4].ports.push(pr("V2", "B"));
    c.in_block = c.components.iter().map(|c| c.id.clone()).collect();
    c.assembly = Some(Assembly {
        block: CatalogueRef::new("blk", 1),
        mounts: mounts
            .iter()
            .map(|(c, f)| (c.to_string(), f.to_string()))
            .collect(),
    });
    (c, cat)
}

#[test]
fn mounted_valves_validate_and_round_trip() {
    let (c, cat) = on_block(&[("V1", "F1"), ("V2", "F2")]);
    assert_eq!(cat.problems(), Vec::<String>::new());
    assert_eq!(c.validate(&cat), vec![]);
    let json = c.to_json();
    assert_eq!(Circuit::from_json(&json).unwrap(), c);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["assembly"]["block"], "blk@1");
    assert_eq!(value["assembly"]["mounts"]["V2"], "F2");
}

#[test]
fn assembly_mistakes_are_reported() {
    let (c, cat) = on_block(&[("V1", "F1"), ("V2", "F1"), ("P", "F2"), ("V9", "F2")]);
    assert_eq!(
        c.validate(&cat),
        vec![
            Issue::MountedPort("P".into()),
            Issue::FaceTaken {
                face: "F1".into(),
                components: ["V1".into(), "V2".into()],
            },
            Issue::MountedUnknownComponent {
                component: "V9".into(),
                face: "F2".into(),
            },
        ]
    );

    let (c, cat) = on_block(&[("V1", "F3"), ("V2", "F7"), ("K1", "F2")]);
    assert_eq!(
        c.validate(&cat),
        vec![
            Issue::PatternMismatch {
                component: "K1".into(),
                face: "F2".into(),
                face_pattern: "ISO 4401-03".into(),
                mounting: None,
            },
            Issue::PatternMismatch {
                component: "V1".into(),
                face: "F3".into(),
                face_pattern: "ISO 4401-05".into(),
                mounting: Some("ISO 4401-03".into()),
            },
            Issue::UnknownFace {
                component: "V2".into(),
                face: "F7".into(),
            },
        ]
    );

    // A subplate valve left off the block is a warning; a line-mounted check valve is not.
    let (c, cat) = on_block(&[("V1", "F1")]);
    let issues = c.validate(&cat);
    assert_eq!(issues, vec![Issue::Unmounted("V2".into())]);
    assert_eq!(issues[0].severity(), Severity::Warning);

    let (c, _) = on_block(&[("V1", "F1")]);
    assert_eq!(
        c.validate(&catalogue()),
        vec![
            Issue::UnknownBlock(CatalogueRef::new("blk", 1)),
            Issue::Unmounted("V2".into())
        ]
    );
}

#[test]
fn only_valves_in_the_block_belong_on_it() {
    // V2 is drawn outside the block, so leaving it off the block is right.
    let (mut c, cat) = on_block(&[("V1", "F1")]);
    c.in_block.remove("V2");
    assert_eq!(c.validate(&cat), vec![]);

    // V2 in the block but not mounted is still a warning.
    c.in_block.insert("V2".into());
    assert_eq!(c.validate(&cat), vec![Issue::Unmounted("V2".into())]);

    // V2 mounted while drawn outside the block: the two views disagree.
    let (mut c, cat) = on_block(&[("V1", "F1"), ("V2", "F2")]);
    c.in_block.remove("V2");
    let issues = c.validate(&cat);
    assert_eq!(issues, vec![Issue::MountedOutsideBlock("V2".into())]);
    assert_eq!(issues[0].severity(), Severity::Warning);
    assert_eq!(
        issues[0].to_string(),
        "V2 is mounted on the block but drawn outside it; tick In block in the schematic, or take it off its face in the 3D tab"
    );

    // Nothing ticked In block: one warning instead of silence or one per valve.
    let (mut c, cat) = on_block(&[("V1", "F1")]);
    c.in_block.clear();
    let issues = c.validate(&cat);
    assert_eq!(issues, vec![Issue::NothingInBlock]);
    assert_eq!(issues[0].severity(), Severity::Warning);
}

#[test]
fn face_transform_puts_the_valve_frame_on_the_face() {
    let apply = |m: [[f64; 4]; 4], p: [f64; 3]| {
        [0, 1, 2].map(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
    };
    let b = block();
    // Top face: the valve's datum lands on the face origin and its body points up.
    let top = b.face("F1").unwrap().transform().unwrap();
    assert_eq!(apply(top, [0.0, 0.0, 0.0]), [45.0, 10.0, 70.0]);
    assert_eq!(apply(top, [1.0, 2.0, 3.0]), [46.0, 12.0, 73.0]);
    // Front face: valve +z goes out of the block (-y), valve +y goes up (+z).
    let front = b.face("F3").unwrap().transform().unwrap();
    assert_eq!(apply(front, [1.0, 2.0, 3.0]), [31.0, -3.0, 17.0]);
    // Axes given at any length are normalised.
    let mut f = face("F", "p", [0.0; 3], [0.0, 0.0, 5.0]);
    f.x_axis = [2.0, 0.0, 0.0];
    assert_eq!(
        apply(f.transform().unwrap(), [1.0, 1.0, 1.0]),
        [1.0, 1.0, 1.0]
    );
}

#[test]
fn bad_blocks_are_problems() {
    let mut cat = catalogue();
    let mut b = block();
    b.mount_faces[1].name = "F1".into();
    b.mount_faces[2].x_axis = [0.0, 1.0, 0.0];
    b.mount_faces.push(face("", "", [0.0; 3], [0.0; 3]));
    cat.blocks.push(b.clone());
    cat.blocks.push(b);
    let problems = cat.problems();
    for expected in [
        "block blk@1 is defined twice",
        "block blk@1: face F1 is defined twice",
        "block blk@1: face F3: x axis is not in the face plane (not perpendicular to the normal)",
        "block blk@1: a mount face has no name",
        "block blk@1: face  has no mounting pattern",
        "block blk@1: face : normal has no direction",
    ] {
        assert!(
            problems.iter().any(|p| p == expected),
            "missing {expected:?} in {problems:#?}"
        );
    }
}
