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
        kind: ComponentKind::Valve {
            valve_type: valve_type.to_string().try_into().unwrap(),
        },
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
        },
    );
    c.layout
        .wires
        .insert("n-p".into(), vec![vec![[0.0, 0.0], [5.0, 0.0]]]);
    c.nets[0].name = Some("pressure".into());
    let json = c.to_json();
    assert_eq!(Circuit::from_json(&json).unwrap(), c);

    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["components"][0]["valve_type"], "rpe3-z11@1");
    assert_eq!(value["layout"]["components"]["V1"]["rotation"], 270);
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
    assert!(matches!(
        Circuit::from_json(r#"{"schema": 2, "name": "x", "components": []}"#),
        Err(LoadError::UnsupportedSchema(2))
    ));
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
