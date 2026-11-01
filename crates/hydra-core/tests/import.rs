//! What stops an imported circuit from being saved.

use hydra_core::*;

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

fn problems(c: &Circuit) -> Vec<String> {
    c.import_problems(&catalogue())
}

fn has(c: &Circuit, needle: &str) {
    let p = problems(c);
    assert!(p.iter().any(|p| p.contains(needle)), "{needle}: {p:?}");
}

fn valve_mut<'a>(c: &'a mut Circuit, id: &str) -> &'a mut Valve {
    match &mut c.components.iter_mut().find(|x| x.id == id).unwrap().kind {
        ComponentKind::Valve(v) => v,
        _ => panic!("{id} is not a valve"),
    }
}

fn item_mut<'a>(c: &'a mut Circuit, id: &str) -> &'a mut Item {
    match &mut c.components.iter_mut().find(|x| x.id == id).unwrap().kind {
        ComponentKind::Item(i) => i,
        _ => panic!("{id} is not an item"),
    }
}

#[test]
fn the_demo_imports_have_no_problems() {
    assert_eq!(problems(&manifold()), Vec::<String>::new());
    let motors = Circuit::from_json(&fixture("circuits/demo-import-motors.json")).unwrap();
    assert_eq!(problems(&motors), Vec::<String>::new());
}

#[test]
fn names_and_sizes_are_bounded() {
    let mut c = manifold();
    c.name = "  ".into();
    has(&c, "needs a name");
    c.name = "x".repeat(MAX_CIRCUIT_NAME + 1);
    has(&c, "longer than 200");
    let mut c = manifold();
    let extra = c.bom_extra[0].clone();
    c.bom_extra = vec![extra; MAX_IMPORT_EXTRA_LINES + 1];
    has(&c, "extra bill-of-materials lines");
    let mut c = manifold();
    let port = c.components[0].clone();
    for i in 0..MAX_IMPORT_COMPONENTS {
        let mut p = port.clone();
        p.id = format!("Q{i}");
        c.layout
            .components
            .insert(p.id.clone(), Placement::default());
        c.components.push(p);
    }
    has(&c, "more than one drawing holds");
}

#[test]
fn errors_from_validation_and_unplaced_parts_stop_it() {
    let mut c = manifold();
    c.nets[0].ports.push(PortRef {
        component: "6.1".into(),
        port: "Z".into(),
    });
    has(&c, "which that component does not have");
    let mut c = manifold();
    c.layout.components.remove("7.1");
    has(&c, "7.1 has no place on the sheet");
}

#[test]
fn a_missing_valve_must_say_what_it_is() {
    let mut c = manifold();
    valve_mut(&mut c, "5.2")
        .requested
        .as_mut()
        .unwrap()
        .function = None;
    has(
        &c,
        "valve 5.2 is missing from the catalogue: give requested.function",
    );
    let mut c = manifold();
    valve_mut(&mut c, "5.2")
        .requested
        .as_mut()
        .unwrap()
        .mounting = None;
    has(&c, "give requested.mounting");
    let mut c = manifold();
    valve_mut(&mut c, "5.2").display = None;
    has(&c, "has no look-alike symbol");
    let mut c = manifold();
    valve_mut(&mut c, "5.2").requested = None;
    has(&c, "needs requested with the code as printed");
    let mut c = manifold();
    valve_mut(&mut c, "9.1").display = Some(SymbolDef::Box {
        label: String::new(),
        ports: vec![],
    });
    has(&c, "valve 9.1 display: a box needs 1 to 8 ports");
}

#[test]
fn codes_must_be_trimmed_and_present() {
    let mut c = manifold();
    valve_mut(&mut c, "6.1").requested.as_mut().unwrap().code = " DEMO ".into();
    has(&c, "has spaces at the start or end");
    let mut c = manifold();
    valve_mut(&mut c, "6.1").requested.as_mut().unwrap().code = "".into();
    has(&c, "valve 6.1 has an empty code");
    let mut c = manifold();
    item_mut(&mut c, "M1").code = Some("".into());
    has(&c, "item M1 has an empty code");
}

#[test]
fn valves_are_not_items_except_a_fixed_orifice() {
    let mut c = manifold();
    item_mut(&mut c, "X1").symbol = SymbolDef::Relief { adjustable: false };
    has(&c, "item X1 is drawn as a valve");
    let mut c = manifold();
    item_mut(&mut c, "X1").symbol = SymbolDef::Orifice { adjustable: true };
    assert_eq!(problems(&c), Vec::<String>::new());
}

#[test]
fn extra_lines_need_a_quantity_and_something_to_order() {
    let mut c = manifold();
    c.bom_extra[0].quantity = 0;
    has(&c, "extra line 1 has quantity 0");
    let mut c = manifold();
    c.bom_extra[1].code = None;
    c.bom_extra[1].description = None;
    has(&c, "extra line 2 needs a code or a description");
}

#[test]
fn notes_are_bounded() {
    let mut c = manifold();
    c.import.as_mut().unwrap().notes = vec!["x".repeat(MAX_PROPOSAL_NOTES + 1)];
    has(&c, "notes are longer");
}

#[test]
fn texts_are_bounded_and_names_trimmed() {
    let long = "x".repeat(MAX_IMPORT_TEXT + 1);
    let mut c = manifold();
    valve_mut(&mut c, "6.1")
        .requested
        .as_mut()
        .unwrap()
        .description = long.clone();
    has(&c, "valve 6.1: description is longer than 200");
    let mut c = manifold();
    item_mut(&mut c, "X1").label = Some(long.clone());
    has(&c, "item X1: label is longer than 200");
    let mut c = manifold();
    c.bom_extra[0].description = Some(long);
    has(&c, "extra line 1: description is longer than 200");
    let mut c = manifold();
    c.name = " DEMO-450".into();
    has(&c, "the name has spaces at the start or end");
}

#[test]
fn a_stand_in_must_say_what_it_stands_in_for() {
    let mut c = manifold();
    valve_mut(&mut c, "3.1").requested = None;
    has(&c, "valve 3.1 is marked as a stand-in, but for what?");
}

#[test]
fn a_listed_requested_part_is_chosen() {
    let cat = catalogue();
    let mut c = manifold();
    let v = valve_mut(&mut c, "6.1");
    v.part = None;
    assert_eq!(
        c.component("6.1").unwrap().valve_status(&cat),
        Some(ValveStatus::PartNotListed)
    );
    c.choose_requested_parts(&cat);
    let v = c.component("6.1").unwrap();
    assert_eq!(
        v.valve().unwrap().part.as_deref(),
        Some("DEMO-06-4-2-S-24DC")
    );
    assert_eq!(v.valve_status(&cat), Some(ValveStatus::Normal));
    // A part the type doesn't list stays unchosen.
    assert_eq!(c.component("5.1").unwrap().valve().unwrap().part, None);
}

#[test]
fn drawn_wires_must_be_well_formed() {
    let wired = |net: &str, wires: Vec<Vec<[f32; 2]>>| {
        let mut c = manifold();
        c.layout.wires.insert(net.into(), wires);
        c
    };
    let net = manifold().nets[0].id.clone();
    let ok = wired(&net, vec![vec![[0.0, 0.0], [0.0, 10.0], [5.0, 10.0]]]);
    assert_eq!(problems(&ok), Vec::<String>::new());

    has(
        &wired("n-nope", vec![]),
        "names net \"n-nope\", which is not in nets",
    );
    has(
        &wired(
            &net,
            vec![vec![[0.0, 0.0], [0.0, 1.0]]; MAX_WIRES_PER_NET + 1],
        ),
        "with 101 wires, more than 100",
    );
    has(&wired(&net, vec![vec![[1.0, 2.0]]]), "has 1 point(s)");
    has(
        &wired(&net, vec![vec![[0.0, 0.0]; MAX_WIRE_POINTS + 1]]),
        "has 101 points, more than 100",
    );
    has(
        &wired(&net, vec![vec![[0.0, 0.0], [0.0, f32::INFINITY]]]),
        "has point 0, inf; keep the layout within",
    );
    has(
        &wired(&net, vec![vec![[0.0, 0.0], [0.0, f32::NAN]]]),
        "has point 0, NaN",
    );
    has(
        &wired(&net, vec![vec![[0.0, 0.0], [MAX_SHEET_MM + 1.0, 0.0]]]),
        "has point 10001, 0",
    );
    has(
        &wired(&net, vec![vec![[0.0, 0.0], [0.0, 5.0], [3.0, 9.0]]]),
        &format!("wire 1 of net {net} runs diagonally from 0, 5 to 3, 9"),
    );
}
