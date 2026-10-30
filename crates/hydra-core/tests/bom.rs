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
    assert_eq!(lines[3].catalogue.to_string(), "blk@1");
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
            "Item,Quantity,Designators,Manufacturer,Description,Part number,Catalogue",
            r#"1,1,K1,Argo-Hytos,"VJ3 06, ""soft""","'=HYPERLINK(""x"")",vt-check@1"#,
            "2,2,V2 V10,Argo-Hytos,RPE3-06 Z11,,vt-spool@1",
            "3,1,X1,,,,vt-gone@3",
            "4,1,,,Germani block,,blk@1",
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
