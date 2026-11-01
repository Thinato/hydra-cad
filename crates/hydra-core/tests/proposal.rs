use hydra_core::*;

fn catalogue() -> Catalogue {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A new demo valve type using an existing symbol, with two parts.
fn proposal() -> Proposal {
    serde_json::from_value(serde_json::json!({
        "valve_type": {
            "key": "vt-demo-new", "version": 0, "manufacturer": "demo",
            "series": "DEMO-10", "code": "4-3-C", "symbol": "sym-4-3-closed-centre@1",
            "mounting": "ISO 4401-05",
            "params": {"function": "directional", "max_pressure_bar": 315}
        },
        "parts": [
            {"part_number": "DEMO-10-24DC", "options": {"voltage": "24 V DC"}},
            {"part_number": "DEMO-10-230AC", "options": {"voltage": "230 V AC"}}
        ],
        "notes": "page 1, ordering code table"
    }))
    .unwrap()
}

#[test]
fn a_good_proposal_has_no_problems_and_applies() {
    let cat = catalogue();
    let p = proposal();
    assert_eq!(p.problems(&cat), Vec::<String>::new());
    assert_eq!(p.target(), CatalogueRef::new("vt-demo-new", 1));
    let after = p.apply(&cat);
    assert_eq!(after.valve_type(&p.target()).unwrap().series, "DEMO-10");
    assert_eq!(after.parts_of(&p.target()).count(), 2);
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Proposal>(&json).unwrap(), p);
}

#[test]
fn updating_keeps_unlisted_parts_and_replaces_listed_ones() {
    let cat = catalogue();
    let mut p = proposal();
    p.valve_type.key = "vt-demo-4-2".into();
    p.valve_type.version = 1;
    p.valve_type.symbol = Some(CatalogueRef::new("sym-4-2-spring-return", 1));
    p.parts = vec![PartDraft {
        part_number: "DEMO-06-4-2-S-24DC".into(),
        options: serde_json::json!({"voltage": "24 V DC", "seals": "FKM"}),
    }];
    assert_eq!(p.problems(&cat), Vec::<String>::new());
    let after = p.apply(&cat);
    let target = p.target();
    let parts: Vec<_> = after.parts_of(&target).collect();
    assert_eq!(parts.len(), 2, "{parts:?}");
    let replaced = parts
        .iter()
        .find(|x| x.part_number == "DEMO-06-4-2-S-24DC")
        .unwrap();
    assert_eq!(replaced.options["seals"], "FKM");
}

#[test]
fn a_new_symbol_is_checked_and_must_be_used() {
    let cat = catalogue();
    let mut p = proposal();
    p.new_symbol = Some(SymbolDefRecord {
        key: "sym-demo-new".into(),
        version: 1,
        definition: serde_json::from_value(serde_json::json!({
            "kind": "directional_valve",
            "ports": [{"name": "A", "side": "top"}, {"name": "P", "side": "bottom"}],
            "positions": [{"flows": [["P", "A"]]}, {"blocked": ["P", "X"]}],
            "normal_position": 1
        }))
        .unwrap(),
    });
    let bad_port = "position 2 refers to port X, which the valve does not have";
    assert_eq!(
        p.problems(&cat),
        [
            "new_symbol is sym-demo-new@1 but the valve type uses sym-4-3-closed-centre@1; \
             point the valve type at the new symbol"
                .to_string(),
            format!("new_symbol sym-demo-new@1: {bad_port}"),
        ]
    );
    p.valve_type.symbol = Some(CatalogueRef::new("sym-demo-new", 1));
    assert_eq!(
        p.problems(&cat),
        [format!("new_symbol sym-demo-new@1: {bad_port}")]
    );
    p.new_symbol.as_mut().unwrap().version = 2;
    p.valve_type.symbol = Some(CatalogueRef::new("sym-demo-new", 2));
    assert_eq!(
        p.problems(&cat),
        [
            "new_symbol sym-demo-new@2 must be version 1".to_string(),
            format!("new_symbol sym-demo-new@2: {bad_port}"),
        ]
    );
    // Fixed, it passes.
    p.new_symbol.as_mut().unwrap().version = 1;
    p.valve_type.symbol = Some(CatalogueRef::new("sym-demo-new", 1));
    let SymbolDef::DirectionalValve(v) = &mut p.new_symbol.as_mut().unwrap().definition else {
        unreachable!()
    };
    v.positions[1].blocked = vec!["P".into(), "A".into()];
    assert_eq!(p.problems(&cat), Vec::<String>::new());
}

#[test]
fn mistakes_are_explained() {
    let cat = catalogue();
    let with = |f: &dyn Fn(&mut Proposal)| {
        let mut p = proposal();
        f(&mut p);
        p.problems(&cat)
    };
    assert_eq!(
        with(&|p| p.valve_type.key = "vt-demo-4-2".into()),
        ["valve type vt-demo-4-2 already exists at version 1; use version 1 to update it"]
    );
    assert_eq!(
        with(&|p| p.valve_type.version = 3),
        ["valve type vt-demo-new doesn't exist yet; use version 0 to propose a new one"]
    );
    assert_eq!(
        with(&|p| p.valve_type.symbol = None),
        ["the valve type needs a symbol: an existing symbol definition, or new_symbol"]
    );
    assert_eq!(
        with(&|p| p.valve_type.manufacturer = "acme".into()),
        ["unknown manufacturer \"acme\"; use one of these keys: argo-hytos, demo"]
    );
    assert_eq!(
        with(&|p| p.valve_type.symbol = Some(CatalogueRef::new("sym-nope", 1))),
        ["unknown symbol sym-nope@1; use an existing symbol definition or define it in new_symbol"]
    );
    assert_eq!(
        with(&|p| p.parts[1].part_number = "DEMO-10-24DC".into()),
        ["part DEMO-10-24DC is listed twice"]
    );
    assert_eq!(
        with(&|p| p.valve_type.code = " ".into()),
        ["the valve type needs a series and a code"]
    );
    assert_eq!(
        with(&|p| p.valve_type.key = "a@b".into()),
        ["valve type key \"a@b\" contains '@'"]
    );
    // Everything at once, so the assistant fixes it in one round.
    let all = with(&|p| {
        p.valve_type.version = 1;
        p.valve_type.manufacturer = "Demo".into();
        p.valve_type.symbol = Some(CatalogueRef::new("sym-nope", 1));
    });
    assert_eq!(all.len(), 3, "{all:#?}");
}

#[test]
fn problems_the_catalogue_already_has_are_not_the_proposals() {
    let mut cat = catalogue();
    cat.parts.push(Part {
        part_number: "ORPHAN".into(),
        valve_type: CatalogueRef::new("vt-gone", 1),
        options: serde_json::Value::Null,
    });
    assert_eq!(cat.problems().len(), 1);
    assert_eq!(proposal().problems(&cat), Vec::<String>::new());
}

#[test]
fn proposals_cannot_take_over_what_others_use() {
    let cat = catalogue();
    let with = |f: &dyn Fn(&mut Proposal)| {
        let mut p = proposal();
        f(&mut p);
        p.problems(&cat)
    };
    // The manufacturer of an existing valve type stays.
    assert_eq!(
        with(&|p| {
            p.valve_type.key = "vt-demo-4-2".into();
            p.valve_type.version = 1;
            p.valve_type.manufacturer = "argo-hytos".into();
        }),
        ["valve type vt-demo-4-2 is made by demo; a proposal can't change the manufacturer"]
    );
    // An existing symbol key is reused, never redefined.
    assert_eq!(
        with(&|p| {
            p.new_symbol = Some(SymbolDefRecord {
                key: "sym-4-3-closed-centre".into(),
                version: 2,
                definition: SymbolDef::Check { spring: false },
            });
            p.valve_type.symbol = Some(CatalogueRef::new("sym-4-3-closed-centre", 2));
        }),
        [
            "symbol key sym-4-3-closed-centre already exists; reuse sym-4-3-closed-centre@1 as \
             the valve type's symbol, or give new_symbol a new key"
        ]
    );
    // A part number belongs to one valve type.
    assert_eq!(
        with(&|p| p.parts[0].part_number = "DEMO-RV-210".into()),
        ["part DEMO-RV-210 already belongs to valve type vt-demo-relief@1"]
    );
    assert_eq!(
        with(&|p| p.parts[0].part_number = " DEMO-10-24DC".into()),
        ["part number \" DEMO-10-24DC\" has spaces at the start or end"]
    );
    assert_eq!(
        with(&|p| p.valve_type.series = "DEMO-10 ".into()),
        ["the series \"DEMO-10 \" has spaces at the start or end"]
    );
}

#[test]
fn oversized_proposals_are_refused() {
    let cat = catalogue();
    let mut p = proposal();
    p.parts = (0..=MAX_PROPOSAL_PARTS)
        .map(|i| PartDraft {
            part_number: format!("P{i}"),
            options: serde_json::Value::Null,
        })
        .collect();
    assert_eq!(
        p.problems(&cat),
        ["501 parts is more than one proposal takes (500)"]
    );
    let mut p = proposal();
    p.notes = "x".repeat(MAX_PROPOSAL_NOTES + 1);
    assert_eq!(
        p.problems(&cat),
        ["the notes are longer than 20000 characters"]
    );
}

#[test]
fn symbols_that_would_draw_wrong_are_refused() {
    let def: SymbolDef = serde_json::from_value(serde_json::json!({
        "kind": "directional_valve",
        "ports": [{"name": "", "side": "top"}, {"name": "A", "side": "top"}, {"name": "P", "side": "bottom"}],
        "positions": [{"flows": [["A", "A"]]}, {"flows": [["P", "A"]], "blocked": ["P"]}],
        "normal_position": 0
    }))
    .unwrap();
    assert_eq!(
        def.problems(),
        [
            "port name \"\" must be 1 to 8 characters without spaces at the ends",
            "position 1 connects port A to itself",
            "position 2 both blocks and connects port P",
        ]
    );
}

#[test]
fn function_ratings_and_cavity_are_checked() {
    let cat = catalogue();
    let with = |params: serde_json::Value| {
        let mut p = proposal();
        p.valve_type.params = params;
        p.problems(&cat)
    };
    let has = |problems: Vec<String>, needle: &str| {
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "{needle}: {problems:?}"
        );
    };
    has(with(serde_json::json!({})), "params.function is missing");
    has(
        with(serde_json::json!({"function": "valve"})),
        "is not one of",
    );
    has(
        with(serde_json::json!({"function": "relief", "max_pressure_bar": "315 bar"})),
        "params: max_pressure_bar must be a positive number",
    );
    has(
        with(serde_json::json!({"function": "relief", "max_flow_l_min": 0})),
        "params: max_flow_l_min must be a positive number",
    );
    has(
        with(serde_json::json!({"function": "relief", "cavity": "yes"})),
        "params.cavity must be true or false",
    );
    has(
        with(serde_json::json!({"function": "relief", "setting_range_bar": [210, 175]})),
        "setting_range_bar must be [min, max]",
    );
    has(
        with(serde_json::json!({"function": "relief", "setting_range_bar": 210})),
        "setting_range_bar must be [min, max]",
    );
    assert_eq!(
        with(
            serde_json::json!({"function": "directional", "cavity": false,
            "setting_range_bar": [1, 2], "max_pressure_bar": 315.5})
        ),
        Vec::<String>::new()
    );
    // A cartridge names its cavity in mounting.
    let mut p = proposal();
    p.valve_type.mounting = None;
    p.valve_type.params = serde_json::json!({"function": "relief", "cavity": true});
    has(
        p.problems(&cat),
        "params.cavity is true but mounting is missing",
    );
    // Parts may state their own setting range, checked the same way.
    let mut p = proposal();
    p.parts[0].options = serde_json::json!({"setting_range_bar": [0, 100]});
    has(p.problems(&cat), "part DEMO-10-24DC: setting_range_bar");
    // An update may rely on the symbol for its function.
    let mut p = proposal();
    p.valve_type = cat
        .valve_type(&CatalogueRef::new("vt-demo-check", 1))
        .unwrap()
        .clone();
    p.parts.clear();
    assert_eq!(p.problems(&cat), Vec::<String>::new());
}

#[test]
fn only_valve_symbols_enter_the_catalogue() {
    let mut p = proposal();
    p.valve_type.symbol = Some(CatalogueRef::new("sym-box-x", 1));
    p.new_symbol = Some(SymbolDefRecord {
        key: "sym-box-x".into(),
        version: 1,
        definition: SymbolDef::Box {
            label: "X".into(),
            ports: vec!["1".into()],
        },
    });
    let problems = p.problems(&catalogue());
    assert!(
        problems.iter().any(|p| p.contains("is not a valve symbol")),
        "{problems:?}"
    );
}

#[test]
fn a_requested_part_is_added_to_an_existing_type() {
    let cat = catalogue();
    let base = cat
        .valve_type(&CatalogueRef::new("vt-demo-check", 1))
        .unwrap()
        .clone();
    let p = Proposal {
        valve_type: base,
        new_symbol: None,
        parts: vec![PartDraft {
            part_number: "DEMO-CK10-05".into(),
            options: serde_json::Value::Null,
        }],
        notes: "page 2, ordering code".into(),
    };
    assert_eq!(p.problems(&cat), Vec::<String>::new());
    let after = p.apply(&cat);
    let v2 = CatalogueRef::new("vt-demo-check", 2);
    assert_eq!(after.parts_of(&v2).count(), 1);
}

#[test]
fn part_ratings_must_be_numbers_too() {
    let mut p = proposal();
    p.parts[1].options = serde_json::json!({"max_pressure_bar": "315 bar"});
    let problems = p.problems(&catalogue());
    assert!(
        problems
            .iter()
            .any(|x| x.contains("part DEMO-10-230AC: max_pressure_bar must be a positive number")),
        "{problems:?}"
    );
}
