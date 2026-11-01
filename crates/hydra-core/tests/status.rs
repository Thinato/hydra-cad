//! Valve status, "now available", stand-in suggestions and the edit counter they are cached by.

use hydra_core::*;
use serde_json::json;

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

fn manifold() -> Circuit {
    Circuit::from_json(&fixture("circuits/demo-import-manifold.json")).unwrap()
}

fn r(s: &str) -> CatalogueRef {
    s.to_string().try_into().unwrap()
}

fn valve(id: &str, v: Valve) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Valve(v),
    }
}

fn requested(code: &str) -> Option<Requested> {
    Some(Requested {
        code: code.into(),
        ..Requested::default()
    })
}

fn circuit(components: Vec<Component>) -> Circuit {
    let mut c = Circuit::from_json(r#"{"schema": 3, "name": "t", "components": []}"#).unwrap();
    c.components = components;
    c
}

fn status(c: &Component, cat: &Catalogue) -> Option<ValveStatus> {
    c.valve_status(cat)
}

#[test]
fn the_demo_import_shows_every_status() {
    let cat = catalogue();
    let s = CircuitStatus::compute(&manifold(), &cat);
    let of = |id: &str| s.status(id);
    assert_eq!(of("6.1"), Some(ValveStatus::Normal));
    assert_eq!(of("7.1"), Some(ValveStatus::Normal));
    assert_eq!(of("5.1"), Some(ValveStatus::PartNotListed));
    assert_eq!(of("5.2"), Some(ValveStatus::Missing));
    assert_eq!(of("9.1"), Some(ValveStatus::Missing));
    assert_eq!(of("3.1"), Some(ValveStatus::StandIn));
    assert_eq!(of("X1"), None);
    assert!(s.issues.contains(&Issue::ValveMissing {
        component: "5.2".into(),
        code: "DEMO-10-4-3-E-24DC".into()
    }));
    assert!(s.issues.iter().all(|i| i.severity() == Severity::Warning));
}

#[test]
fn a_part_that_isnt_listed_is_amber() {
    let cat = catalogue();
    let mut v = Valve {
        valve_type: Some(r("vt-demo-4-2@1")),
        part: Some("DEMO-06-4-2-S-24DC".into()),
        requested: requested("DEMO-06-4-2-S-24DC"),
        ..Valve::default()
    };
    assert_eq!(
        status(&valve("V", v.clone()), &cat),
        Some(ValveStatus::Normal)
    );
    // The part field names it, but the type doesn't list it.
    v.part = Some("DEMO-06-4-2-S-12DC".into());
    v.requested = requested("DEMO-06-4-2-S-12DC");
    assert_eq!(
        status(&valve("V", v.clone()), &cat),
        Some(ValveStatus::PartNotListed)
    );
    // Listed, but not chosen yet.
    v.part = None;
    v.requested = requested("DEMO-06-4-2-S-24DC");
    assert_eq!(
        status(&valve("V", v), &cat),
        Some(ValveStatus::PartNotListed)
    );
    // Nothing requested: whatever is used is fine.
    let plain = valve(
        "V",
        Valve {
            valve_type: Some(r("vt-demo-4-2@1")),
            ..Valve::default()
        },
    );
    assert_eq!(status(&plain, &cat), Some(ValveStatus::Normal));
}

#[test]
fn a_part_added_in_a_new_version_becomes_available_and_swapping_clears_it() {
    let mut cat = catalogue();
    let wanted = "DEMO-06-4-2-S-12DC";
    let mut d = Document::new(circuit(vec![valve(
        "V",
        Valve {
            valve_type: Some(r("vt-demo-4-2@1")),
            requested: requested(wanted),
            ..Valve::default()
        },
    )]));
    let v = |d: &Document| d.circuit.components[0].clone();
    assert_eq!(v(&d).requested_available(&cat), None);

    let mut v2 = cat.valve_type(&r("vt-demo-4-2@1")).unwrap().clone();
    v2.version = 2;
    cat.valve_types.push(v2);
    cat.parts.push(Part {
        part_number: wanted.into(),
        valve_type: r("vt-demo-4-2@2"),
        options: json!(null),
    });
    assert_eq!(status(&v(&d), &cat), Some(ValveStatus::PartNotListed));
    assert_eq!(
        v(&d).requested_available(&cat),
        Some(Available {
            valve_type: r("vt-demo-4-2@2"),
            part: Some(wanted.into())
        })
    );
    let stand_ins = stand_ins(&d.circuit, "V", &cat).unwrap();
    assert!(stand_ins[0].exact && !stand_ins[0].rewire);
    let s = &stand_ins[0];
    d.apply(Command::Use {
        id: "V".into(),
        valve_type: s.valve_type.clone(),
        part: s.part.clone(),
        stand_in: false,
        port_map: s.port_map.clone(),
    })
    .unwrap();
    assert_eq!(status(&v(&d), &cat), Some(ValveStatus::Normal));
    assert_eq!(v(&d).requested_available(&cat), None);
}

#[test]
fn a_missing_valve_whose_key_appears_is_available() {
    let mut cat = catalogue();
    let mut rq = requested("DEMO-NEW-1").unwrap();
    rq.key = Some("vt-demo-new".into());
    let c = valve(
        "R",
        Valve {
            requested: Some(rq),
            display: Some(SymbolDef::Check { spring: false }),
            ..Valve::default()
        },
    );
    assert_eq!(c.requested_available(&cat), None);
    let mut t = cat.valve_type(&r("vt-demo-check@1")).unwrap().clone();
    t.key = "vt-demo-new".into();
    cat.valve_types.push(t);
    assert_eq!(
        c.requested_available(&cat),
        Some(Available {
            valve_type: r("vt-demo-new@1"),
            part: None
        })
    );
}

#[test]
fn a_gone_type_without_a_request_has_no_status() {
    let cat = catalogue();
    let gone = valve(
        "V",
        Valve {
            valve_type: Some(r("vt-gone@1")),
            ..Valve::default()
        },
    );
    assert_eq!(status(&gone, &cat), None);
    let asked = valve(
        "V",
        Valve {
            valve_type: Some(r("vt-gone@1")),
            requested: requested("X"),
            ..Valve::default()
        },
    );
    assert_eq!(status(&asked, &cat), Some(ValveStatus::Missing));
}

fn keys(list: &[StandIn]) -> Vec<String> {
    list.iter()
        .map(|s| match &s.part {
            Some(p) => format!("{}:{p}", s.valve_type),
            None => s.valve_type.to_string(),
        })
        .collect()
}

#[test]
fn directional_stand_ins_need_the_same_spool() {
    let mut cat = catalogue();
    let c = manifold();
    // 5.1 is a tandem-centre valve whose part isn't listed. The only tandem-centre valve is
    // the one it already uses, so there's nothing else to offer.
    assert!(stand_ins(&c, "5.1", &cat).is_err());
    let mut other = cat.valve_type(&r("vt-demo-4-3-tandem@1")).unwrap().clone();
    other.key = "vt-demo-4-3-tandem-b".into();
    cat.valve_types.push(other);
    assert_eq!(
        keys(&stand_ins(&c, "5.1", &cat).unwrap()),
        ["vt-demo-4-3-tandem-b@1"]
    );
    // 5.2 is missing and drawn as a 4/3 closed centre on CETOP 5: nothing in the catalogue.
    let e = stand_ins(&c, "5.2", &cat).unwrap_err();
    assert_eq!(
        e.to_string(),
        "no compatible stand-in: needs directional, ISO 4401-05"
    );
    // The same look-alike on CETOP 3 matches only the closed-centre RPE3, not the 4/2 or tandem.
    let mut c3 = c.clone();
    let ComponentKind::Valve(v) = &mut c3
        .components
        .iter_mut()
        .find(|x| x.id == "5.2")
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    v.requested.as_mut().unwrap().mounting = Some("ISO 4401-03".into());
    assert_eq!(
        keys(&stand_ins(&c3, "5.2", &cat).unwrap()),
        ["vt-rpe3-06-z11@1"]
    );
}

#[test]
fn flow_controls_must_match_actuator_compensation_and_bypass() {
    let mut cat = catalogue();
    let base = cat
        .valve_type(&r("vt-demo-flow-control-prop@1"))
        .unwrap()
        .clone();
    for (key, def) in [
        (
            "manual",
            json!({"kind": "flow_control", "compensated": true, "bypass": true}),
        ),
        (
            "plain",
            json!({"kind": "flow_control", "actuator": "proportional", "bypass": true}),
        ),
        (
            "two-port",
            json!({"kind": "flow_control", "actuator": "proportional", "compensated": true}),
        ),
    ] {
        cat.symbol_defs.push(SymbolDefRecord {
            key: format!("sym-fc-{key}"),
            version: 1,
            definition: serde_json::from_value(def).unwrap(),
        });
        let mut t = base.clone();
        t.key = format!("vt-fc-{key}");
        t.symbol = Some(r(&format!("sym-fc-{key}@1")));
        cat.valve_types.push(t);
    }
    let mut motors = Circuit::from_json(&fixture("circuits/demo-import-motors.json")).unwrap();
    // FC2 is missing, drawn with a second pilot X that no catalogue flow control has.
    assert!(stand_ins(&motors, "FC2", &cat).is_err());
    let Some(Valve {
        display: Some(SymbolDef::FlowControl { pilot_port, .. }),
        ..
    }) = motors
        .components
        .iter_mut()
        .find(|c| c.id == "FC2")
        .and_then(|c| match &mut c.kind {
            ComponentKind::Valve(v) => Some(v),
            _ => None,
        })
    else {
        panic!("FC2 is drawn as a flow control");
    };
    *pilot_port = false;
    // Without it, a proportional compensated 3-port flow control.
    assert_eq!(
        keys(&stand_ins(&motors, "FC2", &cat).unwrap()),
        ["vt-demo-flow-control-prop@1"]
    );
}

fn relief_catalogue() -> Catalogue {
    let mut cat = catalogue();
    let base = cat
        .valve_type(&r("vt-demo-cartridge-relief@1"))
        .unwrap()
        .clone();
    for (key, range) in [("low", [175.0, 210.0]), ("high", [265.0, 320.0])] {
        let mut t = base.clone();
        t.key = format!("vt-cr-{key}");
        t.params["setting_range_bar"] = json!(range);
        cat.valve_types.push(t);
    }
    let mut unknown = base.clone();
    unknown.key = "vt-cr-unknown".into();
    unknown.params = json!({"function": "relief", "cavity": true});
    cat.valve_types.push(unknown);
    cat
}

fn relief_need(setting: Option<f64>, flow: Option<f64>) -> Circuit {
    circuit(vec![valve(
        "R",
        Valve {
            requested: Some(Requested {
                code: "DEMO-WANTED".into(),
                function: Some(ValveFunction::Relief),
                mounting: Some("DEMO-C10-2".into()),
                setting_bar: setting,
                max_flow_l_min: flow,
                ..Requested::default()
            }),
            display: Some(SymbolDef::Relief {
                adjustable: true,
                pilot_port: false,
            }),
            ..Valve::default()
        },
    )])
}

#[test]
fn reliefs_are_compared_by_setting_range() {
    let cat = relief_catalogue();
    let list = keys(&stand_ins(&relief_need(Some(210.0), Some(30.0)), "R", &cat).unwrap());
    assert!(list.contains(&"vt-cr-low@1".to_string()), "{list:?}");
    assert!(!list.contains(&"vt-cr-high@1".to_string()), "{list:?}");
    // Known ratings first, the one leaving them unknown last.
    assert_eq!(list.last().map(String::as_str), Some("vt-cr-unknown@1"));
    // Too little flow is out.
    let list = keys(&stand_ins(&relief_need(Some(210.0), Some(45.0)), "R", &cat).unwrap());
    assert_eq!(list, ["vt-cr-unknown@1"]);
    // An unknown need accepts every relief cartridge.
    let list = keys(&stand_ins(&relief_need(None, None), "R", &cat).unwrap());
    assert_eq!(list.len(), 4, "{list:?}");
}

#[test]
fn a_remote_piloted_relief_only_stands_in_for_one() {
    let mut cat = relief_catalogue();
    let mut t = (cat.valve_type(&r("vt-demo-cartridge-relief@1")))
        .unwrap()
        .clone();
    t.key = "vt-cr-remote".into();
    t.symbol = Some(r("sym-relief-remote@1"));
    cat.valve_types.push(t);
    // A plain relief valve is never offered the one piloted through X...
    let list = keys(&stand_ins(&relief_need(None, None), "R", &cat).unwrap());
    assert_eq!(list.len(), 4, "{list:?}");
    assert!(!list.contains(&"vt-cr-remote@1".to_string()), "{list:?}");
    // ...and one drawn with X is offered only that one.
    let mut c = relief_need(None, None);
    let ComponentKind::Valve(v) = &mut c.components[0].kind else {
        panic!("R is a valve");
    };
    v.display = Some(SymbolDef::Relief {
        adjustable: true,
        pilot_port: true,
    });
    assert_eq!(keys(&stand_ins(&c, "R", &cat).unwrap()), ["vt-cr-remote@1"]);
}

#[test]
fn parts_with_their_own_ratings_are_separate_candidates() {
    let cat = catalogue();
    let need = |setting| {
        circuit(vec![valve(
            "C",
            Valve {
                requested: Some(Requested {
                    code: "DEMO-CB-WANTED".into(),
                    function: Some(ValveFunction::Counterbalance),
                    mounting: Some("DEMO-C10-3".into()),
                    setting_bar: Some(setting),
                    ..Requested::default()
                }),
                display: Some(SymbolDef::Counterbalance),
                ..Valve::default()
            },
        )])
    };
    assert_eq!(
        keys(&stand_ins(&need(180.0), "C", &cat).unwrap()),
        ["vt-demo-counterbalance@1:DEMO-CB10-21"]
    );
    assert_eq!(
        keys(&stand_ins(&need(300.0), "C", &cat).unwrap()),
        ["vt-demo-counterbalance@1:DEMO-CB10-32"]
    );
}

#[test]
fn the_exact_requested_part_comes_first_even_if_it_needs_rewiring() {
    let cat = catalogue();
    // Drawn as a box with ports 1 and 2, but the requested part is a listed relief cartridge.
    let c = circuit(vec![valve(
        "R",
        Valve {
            requested: Some(Requested {
                code: "DEMO-CR10-210".into(),
                function: Some(ValveFunction::Other),
                ..Requested::default()
            }),
            display: Some(SymbolDef::Box {
                label: String::new(),
                ports: vec!["1".into(), "2".into()],
            }),
            ..Valve::default()
        },
    )]);
    let list = stand_ins(&c, "R", &cat).unwrap();
    assert_eq!(keys(&list), ["vt-demo-cartridge-relief@1:DEMO-CR10-210"]);
    assert!(list[0].exact && list[0].rewire);
    assert_eq!(
        list[0].port_map,
        [
            ("1".to_string(), "P".to_string()),
            ("2".to_string(), "T".to_string())
        ]
    );
}

#[test]
fn no_function_means_nothing_to_suggest() {
    let c = circuit(vec![valve(
        "R",
        Valve {
            requested: requested("DEMO-UNKNOWN"),
            display: Some(SymbolDef::Check { spring: false }),
            ..Valve::default()
        },
    )]);
    let e = stand_ins(&c, "R", &catalogue()).unwrap_err();
    assert_eq!(
        e.to_string(),
        "no compatible stand-in: needs a known function, no mounting"
    );
    assert!(stand_ins(&c, "nope", &catalogue()).is_err());
}

#[test]
fn the_edit_counter_moves_only_on_changes() {
    let mut d = Document::new(manifold());
    assert_eq!(d.generation(), 0);
    assert!(d.apply(Command::ClearImport).is_ok());
    assert_eq!(d.generation(), 1);
    assert!(d.apply(Command::ClearImport).is_err());
    assert_eq!(d.generation(), 1);
    d.undo();
    d.redo();
    assert_eq!(d.generation(), 3);
    assert!(d.undo().is_some() && d.undo().is_none());
    assert_eq!(d.generation(), 4);
}
