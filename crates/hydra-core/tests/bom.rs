use hydra_core::*;
use serde_json::json;

fn catalogue() -> Catalogue {
    serde_json::from_value(json!({
        "manufacturers": [{"key": "argo", "name": "Argo-Hytos"}],
        "symbol_defs": [{"key": "sym-check", "version": 1, "definition": {"kind": "check", "spring": true}}],
        "valve_types": [
            {"key": "vt-check", "version": 1, "manufacturer": "argo", "series": "VJ3", "code": "06, \"soft\"",
             "symbol": "sym-check@1"},
            {"key": "vt-spool", "version": 1, "manufacturer": "argo", "series": "RPE3-06", "code": "Z11",
             "symbol": "sym-check@1"}
        ],
        "parts": [
            {"part_number": "=HYPERLINK(\"x\")", "valve_type": "vt-check@1"},
            {"part_number": "RPE3-063Z11/02400E1", "valve_type": "vt-spool@1"},
            {"part_number": "RPE3-063Z11/01200E1", "valve_type": "vt-spool@1"}
        ],
        "blocks": [{"key": "blk", "version": 1, "name": "Germani block"}]
    }))
    .unwrap()
}

fn circuit() -> Circuit {
    serde_json::from_value(json!({
        "schema": 2,
        "name": "bom",
        "components": [
            {"id": "V10", "kind": "valve", "valve_type": "vt-spool@1"},
            {"id": "P", "kind": "port", "label": "P"},
            {"id": "K1", "kind": "valve", "valve_type": "vt-check@1"},
            {"id": "V2", "kind": "valve", "valve_type": "vt-spool@1"},
            {"id": "X1", "kind": "valve", "valve_type": "vt-gone@3"}
        ],
        "assembly": {"block": "blk@1"}
    }))
    .unwrap()
}

#[test]
fn bom_groups_by_valve_type_in_designator_order() {
    let lines = circuit().bom(&catalogue());
    let summary: Vec<_> = lines
        .iter()
        .map(|l| {
            (
                l.designators.iter().map(String::as_str).collect::<Vec<_>>(),
                l.quantity,
                l.description.as_str(),
                l.part_number.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (vec!["K1"], 1, "VJ3 06, \"soft\"", Some("=HYPERLINK(\"x\")")),
            // Two orderable parts: the engineer picks one.
            (vec!["V2", "V10"], 2, "RPE3-06 Z11", None),
            // Unknown rows stay on the list instead of vanishing.
            (vec!["X1"], 1, "", None),
            (vec![], 1, "Germani block", None),
        ]
    );
    assert_eq!(lines[0].manufacturer, "Argo-Hytos");
    assert_eq!(lines[3].catalogue.as_ref().unwrap().to_string(), "blk@1");
}

#[test]
fn csv_quotes_fields_and_defuses_formulas() {
    let csv = bom_csv(&circuit().bom(&catalogue()));
    let rows: Vec<&str> = csv
        .strip_prefix('\u{feff}')
        .unwrap()
        .split("\r\n")
        .collect();
    assert_eq!(
        rows,
        [
            "Item,Position,Quantity,Designators,Manufacturer,Description,Part number,Catalogue,Status,Requested",
            r#"1,,1,K1,Argo-Hytos,"VJ3 06, ""soft""","'=HYPERLINK(""x"")",vt-check@1,,"#,
            "2,,2,V2 V10,Argo-Hytos,RPE3-06 Z11,,vt-spool@1,,",
            "3,,1,X1,,,,vt-gone@3,,",
            "4,,1,,,Germani block,,blk@1,,",
            "",
        ]
    );
}

#[test]
fn empty_circuit_has_only_a_header() {
    let empty = Circuit::from_json(r#"{"schema": 2, "name": "e", "components": []}"#).unwrap();
    assert!(empty.bom(&catalogue()).is_empty());
    assert_eq!(bom_csv(&[]).lines().count(), 1);
}

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn imported_circuits_list_status_requests_items_and_extras() {
    let cat: Catalogue = serde_json::from_str(&fixture("catalogue.json")).unwrap();
    let c = Circuit::from_json(&fixture("circuits/demo-import-manifold.json")).unwrap();
    let lines = c.bom(&cat);
    let find = |d: &str| {
        lines
            .iter()
            .find(|l| l.designators.iter().any(|x| x == d))
            .unwrap_or_else(|| panic!("no line for {d}"))
    };
    // A stand-in shows both what it is and what was asked for.
    let k = find("3.1");
    assert_eq!(k.status, Some(ValveStatus::StandIn));
    assert_eq!(k.requested.as_deref(), Some("DEMO-CK10-05"));
    assert_eq!(k.catalogue.as_ref().unwrap().to_string(), "vt-demo-check@1");
    // A missing valve is described from the drawing and has no catalogue row.
    let m = find("5.2");
    assert_eq!(m.status, Some(ValveStatus::Missing));
    assert_eq!(m.catalogue, None);
    assert_eq!(m.description, "Directional valve CETOP 5, closed centre");
    assert_eq!(m.requested.as_deref(), Some("DEMO-10-4-3-E-24DC"));
    // Using the requested part: nothing extra to say.
    let n = find("6.1");
    assert_eq!((n.status, n.requested.as_deref()), (None, None));
    assert_eq!(n.part_number.as_deref(), Some("DEMO-06-4-2-S-24DC"));
    assert_eq!(find("5.1").status, Some(ValveStatus::PartNotListed));
    // Coded items are lines once; uncoded items aren't.
    let x2 = find("X2");
    assert_eq!(x2.part_number.as_deref(), Some("DEMO-GIC-10"));
    assert_eq!(x2.position.as_deref(), Some("10"));
    assert!(
        lines
            .iter()
            .all(|l| !l.designators.contains(&"X1".to_string()))
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.part_number.as_deref() == Some("DEMO-MP-04"))
            .count(),
        1
    );
    // Extra lines come last, in drawing order.
    let tail: Vec<_> = lines[lines.len() - 2..]
        .iter()
        .map(|l| (l.part_number.as_deref(), l.quantity, l.position.as_deref()))
        .collect();
    assert_eq!(
        tail,
        [
            (Some("DEMO-BLOCK-450"), 1, Some("1")),
            (Some("DEMO-PLUG-G1/4"), 4, Some("20"))
        ]
    );
    let csv = bom_csv(&lines);
    assert!(csv.contains(",MISSING,DEMO-10-4-3-E-24DC"), "{csv}");
    assert!(csv.contains(",STAND-IN,DEMO-CK10-05"), "{csv}");
}

#[test]
fn a_missing_valve_needs_no_catalogue_at_all() {
    let c = Circuit::from_json(
        r#"{"schema": 3, "name": "x", "components": [{"id": "R1", "kind": "valve",
            "requested": {"code": "-X1", "description": ""}, "display": {"kind": "check"}}]}"#,
    )
    .unwrap();
    let lines = c.bom(&Catalogue::default());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].description, "-X1");
    // Codes from drawings are defused like catalogue text.
    assert!(bom_csv(&lines).contains("'-X1"));
}

#[test]
fn items_with_the_same_code_keep_their_positions() {
    let c = Circuit::from_json(
        r#"{"schema": 3, "name": "x", "components": [
          {"id": "M1", "kind": "item", "symbol": {"kind": "test_point"}, "code": "MP", "position": "9"},
          {"id": "M2", "kind": "item", "symbol": {"kind": "test_point"}, "code": "MP", "position": "9"},
          {"id": "M3", "kind": "item", "symbol": {"kind": "test_point"}, "code": "MP", "position": "12"}]}"#,
    )
    .unwrap();
    let lines = c.bom(&Catalogue::default());
    let rows: Vec<_> = lines
        .iter()
        .map(|l| (l.position.as_deref(), l.quantity))
        .collect();
    assert_eq!(rows, [(Some("9"), 2), (Some("12"), 1)]);
}
