//! Schema 3: valves missing from the catalogue, generic items, stand-in swaps and the
//! catalogue's function and rating helpers.

use hydra_core::{
    Catalogue, CatalogueRef, Circuit, Command, Component, ComponentKind, Document, FlowActuator,
    Issue, Item, PortRef, Ratings, Requested, SCHEMA_VERSION, SymbolDef, Valve, ValveFunction,
};

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn catalogue() -> Catalogue {
    serde_json::from_str(&fixture("catalogue.json")).unwrap()
}

fn r(s: &str) -> CatalogueRef {
    s.to_string().try_into().unwrap()
}

fn pr(component: &str, port: &str) -> PortRef {
    PortRef {
        component: component.into(),
        port: port.into(),
    }
}

#[test]
fn every_fixture_circuit_loads_and_v3_round_trips() {
    let demo = Circuit::from_json(&fixture("circuits/demo-manifold.json")).unwrap();
    assert_eq!(demo.schema, SCHEMA_VERSION);
    assert!(demo.import.is_none() && demo.bom_extra.is_empty());
    for name in ["demo-import-manifold", "demo-import-motors"] {
        let json = fixture(&format!("circuits/{name}.json"));
        let c = Circuit::from_json(&json).unwrap();
        assert!(c.import.is_some(), "{name} is an import");
        assert!(!c.bom_extra.is_empty(), "{name} has extra lines");
        assert_eq!(Circuit::from_json(&c.to_json()).unwrap(), c, "{name}");
        assert_eq!(c.validate(&catalogue()), vec![], "{name}");
    }
}

#[test]
fn an_old_valve_reads_as_one_that_uses_its_type() {
    let c = Circuit::from_json(
        r#"{"schema": 2, "name": "x", "components": [
            {"id": "V1", "kind": "valve", "valve_type": "vt-demo-4-2@1"}]}"#,
    )
    .unwrap();
    // A version 2 document is upgraded on load.
    assert_eq!(c.schema, SCHEMA_VERSION);
    assert_eq!(
        c.components[0].kind,
        ComponentKind::valve(r("vt-demo-4-2@1"))
    );
    // Nothing new is written for it.
    assert!(!c.to_json().contains("stand_in"));
}

#[test]
fn new_symbol_kinds_have_their_ports() {
    let fc = |bypass| SymbolDef::FlowControl {
        actuator: FlowActuator::Proportional,
        compensated: true,
        bypass,
    };
    assert_eq!(fc(true).ports(), ["In", "Out", "Bypass"]);
    assert_eq!(fc(false).ports(), ["In", "Out"]);
    assert_eq!(
        SymbolDef::PilotCheck { spring: true }.ports(),
        ["A", "B", "X"]
    );
    assert_eq!(SymbolDef::Counterbalance.ports(), ["V", "C", "X"]);
    assert_eq!(SymbolDef::TestPoint.ports(), ["M"]);
    let motor = |drain| SymbolDef::HydraulicMotor {
        bidirectional: false,
        variable: false,
        drain,
    };
    assert_eq!(motor(true).ports(), ["A", "B", "L"]);
    assert_eq!(motor(false).ports(), ["A", "B"]);
    assert_eq!(SymbolDef::Pump { variable: false }.ports(), ["P", "S"]);

    assert!(fc(true).is_valve() && SymbolDef::Counterbalance.is_valve());
    assert!(SymbolDef::Orifice { adjustable: false }.is_valve());
    assert!(!SymbolDef::TestPoint.is_valve() && !motor(false).is_valve());
    assert!(!SymbolDef::Pump { variable: false }.is_valve());
}

#[test]
fn a_box_needs_sane_ports_and_label() {
    let b = |label: &str, ports: &[&str]| SymbolDef::Box {
        label: label.into(),
        ports: ports.iter().map(|p| p.to_string()).collect(),
    };
    assert_eq!(b("PB10", &["1", "2", "3"]).problems(), Vec::<String>::new());
    assert_eq!(b("PB10", &["1", "2", "3"]).ports(), ["1", "2", "3"]);
    assert!(!b("x", &[]).problems().is_empty());
    assert!(!b("x", &["1", "1"]).problems().is_empty());
    assert!(!b("x", &[" A"]).problems().is_empty());
    assert!(!b(&"x".repeat(41), &["1"]).problems().is_empty());
    let parsed: SymbolDef = serde_json::from_str(r#"{"kind": "box", "ports": ["1"]}"#).unwrap();
    assert_eq!(parsed, b("", &["1"]));
}

#[test]
fn function_falls_back_to_the_symbol() {
    let c = catalogue();
    for v in &c.valve_types {
        let f = c.function_of(&v.reference());
        assert!(f.is_some(), "{} has a function", v.reference());
    }
    let f = |s: &str| c.function_of(&r(s)).unwrap();
    assert_eq!(f("vt-demo-4-2@1"), ValveFunction::Directional);
    assert_eq!(f("vt-demo-check@1"), ValveFunction::Check);
    assert_eq!(f("vt-demo-relief@1"), ValveFunction::Relief);
    assert_eq!(f("vt-demo-throttle@1"), ValveFunction::FlowControl);
    // Stated in params, it wins.
    assert_eq!(
        f("vt-demo-pilot-check@1"),
        ValveFunction::PilotOperatedCheck
    );
    assert_eq!(c.function_of(&r("vt-nope@1")), None);
    assert_eq!(
        ValveFunction::from_name("pilot_operated_check"),
        Some(ValveFunction::PilotOperatedCheck)
    );
    assert_eq!(ValveFunction::from_name("valve"), None);
}

#[test]
fn ratings_read_params_and_parts_override_them() {
    let c = catalogue();
    let cb = r("vt-demo-counterbalance@1");
    assert_eq!(
        c.ratings_of(&cb, None),
        Ratings {
            max_pressure_bar: Some(420.0),
            max_flow_l_min: Some(40.0),
            setting_range_bar: None,
        }
    );
    assert_eq!(
        c.ratings_of(&cb, Some("DEMO-CB10-32")).setting_range_bar,
        Some([265.0, 320.0])
    );
    assert_eq!(
        c.ratings_of(&cb, Some("NOT-LISTED")),
        c.ratings_of(&cb, None)
    );
    // Malformed values read as unknown.
    let odd = Ratings::from_json(&serde_json::json!({
        "max_pressure_bar": "315 bar", "max_flow_l_min": 30, "setting_range_bar": [1]
    }));
    assert_eq!(
        odd,
        Ratings {
            max_pressure_bar: None,
            max_flow_l_min: Some(30.0),
            setting_range_bar: None,
        }
    );
    assert!(
        c.valve_type(&r("vt-demo-cartridge-relief@1"))
            .unwrap()
            .is_cartridge()
    );
    assert!(!c.valve_type(&r("vt-demo-4-2@1")).unwrap().is_cartridge());
}

#[test]
fn latest_versions_pick_the_highest() {
    let mut c = catalogue();
    let mut v2 = c.valve_type(&r("vt-demo-check@1")).unwrap().clone();
    v2.version = 2;
    c.valve_types.push(v2);
    let latest = c.latest_valve_types();
    assert_eq!(latest["vt-demo-check"].version, 2);
    assert_eq!(latest["vt-demo-4-2"].version, 1);
}

fn red(id: &str, display: Option<SymbolDef>) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Valve(Valve {
            requested: Some(Requested {
                code: "DEMO-X".into(),
                ..Requested::default()
            }),
            display,
            ..Valve::default()
        }),
    }
}

fn bare(components: Vec<Component>) -> Circuit {
    let mut c = Circuit::from_json(r#"{"schema": 3, "name": "t", "components": []}"#).unwrap();
    c.components = components;
    c
}

#[test]
fn a_missing_valve_draws_its_look_alike() {
    let cat = catalogue();
    let c = bare(vec![red("R1", Some(SymbolDef::Check { spring: false }))]);
    assert_eq!(c.components[0].ports(&cat), Some(vec!["A", "B"]));
    // Only its unconnected ports are reported, not the catalogue.
    assert!(
        c.validate(&cat)
            .iter()
            .all(|i| matches!(i, Issue::Unconnected(_)))
    );

    let c = bare(vec![red("R1", None)]);
    assert_eq!(c.components[0].ports(&cat), None);
    assert_eq!(c.validate(&cat), vec![Issue::NoDisplaySymbol("R1".into())]);
}

#[test]
fn a_type_missing_from_the_catalogue_stays_an_error() {
    let cat = catalogue();
    let mut v = red("V1", Some(SymbolDef::Check { spring: false }));
    let ComponentKind::Valve(valve) = &mut v.kind else {
        unreachable!()
    };
    valve.valve_type = Some(r("vt-gone@1"));
    let issues = bare(vec![v]).validate(&cat);
    assert!(issues.contains(&Issue::UnknownValveType {
        component: "V1".into(),
        valve_type: r("vt-gone@1"),
    }));
}

#[test]
fn items_expose_their_symbol_ports_and_never_mount() {
    let cat = catalogue();
    let item = Component {
        id: "M1".into(),
        kind: ComponentKind::Item(Item {
            symbol: SymbolDef::TestPoint,
            label: Some("MP".into()),
            code: None,
            description: None,
            position: None,
        }),
    };
    assert_eq!(item.ports(&cat), Some(vec!["M"]));
    let mut d = Document::new(bare(vec![item]));
    d.apply(Command::SetBlock {
        block: Some(r("blk-demo@2")),
    })
    .unwrap();
    let err = d
        .apply(Command::Mount {
            component: "M1".into(),
            face: Some("C1".into()),
        })
        .unwrap_err();
    assert!(err.contains("not a valve"), "{err}");
    d.apply(Command::SetLabel {
        id: "M1".into(),
        label: "MP SAE-04".into(),
    })
    .unwrap();
    let ComponentKind::Item(i) = &d.circuit.components[0].kind else {
        unreachable!()
    };
    assert_eq!(i.label.as_deref(), Some("MP SAE-04"));
}

#[test]
fn default_port_map_matches_names_then_order() {
    let m = Command::default_port_map(&["1", "2", "3"], &["V", "C", "X"]);
    let pairs: Vec<(&str, &str)> = m.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(pairs, [("1", "V"), ("2", "C"), ("3", "X")]);
    let m = Command::default_port_map(&["A", "B", "X"], &["A", "B"]);
    let pairs: Vec<(&str, &str)> = m.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(pairs, [("A", "A"), ("B", "B")]);
    // Counts left over differ: only the names that match.
    let m = Command::default_port_map(&["P", "1", "2"], &["P", "T"]);
    assert_eq!(m, [("P".to_string(), "P".to_string())]);
}

#[test]
fn use_rewires_ports_keeps_the_request_and_undoes() {
    let json = fixture("circuits/demo-import-manifold.json");
    let mut d = Document::new(Circuit::from_json(&json).unwrap());
    let before = d.circuit.clone();
    // The box look-alike 9.1 (ports 1, 2, 3) becomes a counterbalance (V, C, X).
    d.apply(Command::Use {
        id: "9.1".into(),
        valve_type: r("vt-demo-counterbalance@1"),
        part: Some("DEMO-CB10-21".into()),
        stand_in: true,
        port_map: vec![("1".into(), "V".into()), ("2".into(), "C".into())],
    })
    .unwrap();
    let nets_of = |c: &Circuit, port: &PortRef| {
        c.nets
            .iter()
            .filter(|n| n.ports.contains(port))
            .map(|n| n.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(nets_of(&d.circuit, &pr("9.1", "V")), ["n-p"]);
    assert_eq!(nets_of(&d.circuit, &pr("9.1", "C")), ["n-31"]);
    // Port 3 wasn't mapped, so it came off its net.
    assert!(nets_of(&d.circuit, &pr("9.1", "3")).is_empty());
    assert!(nets_of(&d.circuit, &pr("9.1", "1")).is_empty());
    let v = d.circuit.component("9.1").unwrap().valve().unwrap();
    assert_eq!(v.valve_type, Some(r("vt-demo-counterbalance@1")));
    assert_eq!(v.part.as_deref(), Some("DEMO-CB10-21"));
    assert!(v.stand_in);
    assert_eq!(v.requested.as_ref().unwrap().code, "DEMO-PB10");

    assert_eq!(
        d.undo().as_deref(),
        Some("Use vt-demo-counterbalance@1 as a stand-in for 9.1")
    );
    assert_eq!(d.circuit, before);
}

#[test]
fn use_refuses_items_clashes_and_no_change() {
    let json = fixture("circuits/demo-import-manifold.json");
    let mut c = Circuit::from_json(&json).unwrap();
    let use_ = |id: &str, map: &[(&str, &str)]| Command::Use {
        id: id.into(),
        valve_type: r("vt-demo-4-2@1"),
        part: Some("DEMO-06-4-2-S-24DC".into()),
        stand_in: false,
        port_map: map
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    };
    let before = c.clone();
    assert!(use_("X1", &[]).apply(&mut c).is_err());
    assert!(
        use_("5.2", &[("A", "P"), ("B", "P")])
            .apply(&mut c)
            .is_err()
    );
    let same = [("A", "A"), ("B", "B"), ("P", "P"), ("T", "T")];
    assert!(use_("6.1", &same).apply(&mut c).is_err());
    assert_eq!(c, before);
}

#[test]
fn clear_import_drops_the_import_once() {
    let json = fixture("circuits/demo-import-motors.json");
    let mut c = Circuit::from_json(&json).unwrap();
    Command::ClearImport.apply(&mut c).unwrap();
    assert!(c.import.is_none());
    assert!(Command::ClearImport.apply(&mut c).is_err());
}

#[test]
fn a_cartridge_fits_only_its_cavity() {
    let cat = catalogue();
    let mut c = bare(vec![Component {
        id: "7.1".into(),
        kind: ComponentKind::valve(r("vt-demo-cartridge-relief@1")),
    }]);
    c.in_block.insert("7.1".into());
    let mut doc = Document::new(c.clone());
    doc.apply(Command::SetBlock {
        block: Some(r("blk-demo@2")),
    })
    .unwrap();
    doc.apply(Command::Mount {
        component: "7.1".into(),
        face: Some("C1".into()),
    })
    .unwrap();
    let mounts = |c: &Circuit| {
        c.validate(&cat)
            .into_iter()
            .filter(|i| !matches!(i, Issue::Unconnected(_)))
            .collect::<Vec<_>>()
    };
    assert_eq!(mounts(&doc.circuit), vec![]);
    c = doc.circuit.clone();
    c.assembly
        .as_mut()
        .unwrap()
        .mounts
        .insert("7.1".into(), "F1".into());
    assert!(matches!(
        mounts(&c).as_slice(),
        [Issue::PatternMismatch { face, .. }] if face == "F1"
    ));
}
