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
            id: "argo".into(),
            name: "Argo-Hytos".into(),
        }],
        symbol_defs: vec![
            SymbolDefRecord {
                id: "sym-4-3".into(),
                version: 1,
                definition: spool_4_3(),
            },
            SymbolDefRecord {
                id: "sym-check".into(),
                version: 1,
                definition: SymbolDef::Check { spring: true },
            },
        ],
        valve_types: vec![
            ValveType {
                id: "rpe3-z11".into(),
                manufacturer_id: "argo".into(),
                series: "RPE3-06".into(),
                code: "Z11".into(),
                symbol_def_id: "sym-4-3".into(),
                mounting: Some("ISO 4401-03".into()),
                params: serde_json::Value::Null,
            },
            ValveType {
                id: "check".into(),
                manufacturer_id: "argo".into(),
                series: "VJ3".into(),
                code: "10".into(),
                symbol_def_id: "sym-check".into(),
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
            valve_type: valve_type.into(),
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
            valve("V1", "rpe3-z11"),
            valve("K1", "check"),
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
    assert_eq!(value["components"][0]["valve_type"], "rpe3-z11");
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
    c.components.push(valve("V2", "nope"));
    assert_eq!(
        c.validate(&catalogue()),
        vec![Issue::UnknownValveType {
            component: "V2".into(),
            valve_type: "nope".into()
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
    cat.valve_types[0].manufacturer_id = "bosch".into();
    cat.valve_types[1].symbol_def_id = "gone".into();
    assert_eq!(
        cat.problems(),
        [
            "valve type rpe3-z11: unknown manufacturer bosch",
            "valve type check: unknown symbol gone"
        ]
    );
}
