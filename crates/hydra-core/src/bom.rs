//! Bill of materials: what to order to build a circuit.

use crate::{Catalogue, CatalogueRef, Circuit, ComponentKind};

/// Every component of one valve type version, or the circuit's block.
#[derive(Debug, Clone, PartialEq)]
pub struct BomLine {
    /// Designators in natural order (`V2` before `V10`); empty for the block.
    pub designators: Vec<String>,
    pub quantity: usize,
    /// Manufacturer name; empty when the catalogue doesn't know the row.
    pub manufacturer: String,
    /// Series and code of a valve type, or the block's name.
    pub description: String,
    /// The part number, when the valve type has exactly one; otherwise the engineer picks one.
    pub part_number: Option<String>,
    pub catalogue: CatalogueRef,
}

impl Circuit {
    /// One line per valve type version used, ordered by first designator, then the block.
    /// External ports aren't parts and are left out.
    pub fn bom(&self, catalogue: &Catalogue) -> Vec<BomLine> {
        let mut lines: Vec<BomLine> = Vec::new();
        for c in &self.components {
            let ComponentKind::Valve { valve_type } = &c.kind else {
                continue;
            };
            if let Some(line) = lines.iter_mut().find(|l| &l.catalogue == valve_type) {
                line.designators.push(c.id.clone());
                continue;
            }
            let v = catalogue.valve_type(valve_type);
            let mut parts = catalogue.parts_of(valve_type);
            let part_number = match (parts.next(), parts.next()) {
                (Some(p), None) => Some(p.part_number.clone()),
                _ => None,
            };
            lines.push(BomLine {
                designators: vec![c.id.clone()],
                quantity: 0,
                manufacturer: v
                    .and_then(|v| {
                        catalogue
                            .manufacturers
                            .iter()
                            .find(|m| m.key == v.manufacturer)
                    })
                    .map(|m| m.name.clone())
                    .unwrap_or_default(),
                description: v
                    .map(|v| format!("{} {}", v.series, v.code))
                    .unwrap_or_default(),
                part_number,
                catalogue: valve_type.clone(),
            });
        }
        for line in &mut lines {
            line.designators.sort_by(|a, b| natural(a).cmp(&natural(b)));
            line.quantity = line.designators.len();
        }
        lines.sort_by(|a, b| natural(&a.designators[0]).cmp(&natural(&b.designators[0])));
        if let Some(a) = &self.assembly {
            lines.push(BomLine {
                designators: Vec::new(),
                quantity: 1,
                manufacturer: String::new(),
                description: catalogue
                    .block(&a.block)
                    .map(|b| b.name.clone())
                    .unwrap_or_default(),
                part_number: None,
                catalogue: a.block.clone(),
            });
        }
        lines
    }
}

/// `V10` sorts after `V2`: letters, then the trailing number by value.
fn natural(designator: &str) -> (&str, u64, &str) {
    let digits = designator.len()
        - designator
            .bytes()
            .rev()
            .take_while(u8::is_ascii_digit)
            .count();
    let (prefix, number) = designator.split_at(digits);
    (prefix, number.parse().unwrap_or(0), number)
}

/// The BOM as CSV (RFC 4180) with a UTF-8 byte order mark, so Excel reads accents right.
pub fn bom_csv(lines: &[BomLine]) -> String {
    let mut out = String::from("\u{feff}");
    let header = [
        "Item",
        "Quantity",
        "Designators",
        "Manufacturer",
        "Description",
        "Part number",
        "Catalogue",
    ];
    row(&mut out, header.map(String::from));
    for (i, l) in lines.iter().enumerate() {
        row(
            &mut out,
            [
                (i + 1).to_string(),
                l.quantity.to_string(),
                l.designators.join(" "),
                l.manufacturer.clone(),
                l.description.clone(),
                l.part_number.clone().unwrap_or_default(),
                l.catalogue.to_string(),
            ],
        );
    }
    out
}

fn row<const N: usize>(out: &mut String, fields: [String; N]) {
    let fields: Vec<String> = fields.iter().map(|f| field(f)).collect();
    out.push_str(&fields.join(","));
    out.push_str("\r\n");
}

fn field(text: &str) -> String {
    // Catalogue text can come from an AI-read datasheet: never let a spreadsheet run it.
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}
