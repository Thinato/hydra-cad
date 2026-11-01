//! Bill of materials: what to order to build a circuit.

use crate::{Catalogue, CatalogueRef, Circuit, ComponentKind, ValveStatus};

/// Every component of one valve type version, part and request; a coded item; the block; or an
/// extra line from the drawing's parts table.
#[derive(Debug, Clone, PartialEq)]
pub struct BomLine {
    /// Designators in natural order (`V2` before `V10`); empty for the block and extra lines.
    pub designators: Vec<String>,
    pub quantity: usize,
    /// Manufacturer name; empty when the catalogue doesn't know the row.
    pub manufacturer: String,
    /// Series and code of a valve type, the block's name, or the drawing's description.
    pub description: String,
    /// The part number used or, failing that, the valve type's only one; otherwise the engineer
    /// picks one.
    pub part_number: Option<String>,
    /// The catalogue row, `None` for parts that aren't in the catalogue.
    pub catalogue: Option<CatalogueRef>,
    /// What the drawing asked for, when it differs from what is used.
    pub requested: Option<String>,
    /// How the valves stand against the catalogue; `None` when normal and for other lines.
    pub status: Option<ValveStatus>,
    /// Position number in the drawing's parts table.
    pub position: Option<String>,
}

impl Circuit {
    /// Valves and coded items ordered by first designator, then the block, then extra lines.
    /// External ports and items without a part number aren't parts and are left out.
    pub fn bom(&self, catalogue: &Catalogue) -> Vec<BomLine> {
        let mut lines: Vec<BomLine> = Vec::new();
        for c in &self.components {
            let line = match &c.kind {
                ComponentKind::Valve(v) => {
                    let status = c
                        .valve_status(catalogue)
                        .filter(|s| *s != ValveStatus::Normal);
                    let requested = v.requested.as_ref();
                    let code = requested.map(|r| r.code.trim().to_string());
                    let known = v.valve_type.as_ref().and_then(|t| catalogue.valve_type(t));
                    let part_number = v.part.clone().or_else(|| {
                        let t = v.valve_type.as_ref()?;
                        let mut parts = catalogue.parts_of(t);
                        match (parts.next(), parts.next()) {
                            (Some(p), None) => Some(p.part_number.clone()),
                            _ => None,
                        }
                    });
                    BomLine {
                        designators: vec![c.id.clone()],
                        quantity: 0,
                        manufacturer: known
                            .and_then(|v| {
                                catalogue
                                    .manufacturers
                                    .iter()
                                    .find(|m| m.key == v.manufacturer)
                            })
                            .map(|m| m.name.clone())
                            .unwrap_or_default(),
                        description: match known {
                            Some(v) => format!("{} {}", v.series, v.code),
                            None => requested
                                .map(|r| {
                                    if r.description.is_empty() {
                                        r.code.clone()
                                    } else {
                                        r.description.clone()
                                    }
                                })
                                .unwrap_or_default(),
                        },
                        // The request is worth printing only where it isn't what is used.
                        requested: code.filter(|c| part_number.as_deref() != Some(c.as_str())),
                        part_number,
                        catalogue: v.valve_type.clone(),
                        status,
                        position: None,
                    }
                }
                ComponentKind::Item(item) => {
                    let Some(code) = item.code.as_ref().map(|c| c.trim().to_string()) else {
                        continue;
                    };
                    BomLine {
                        designators: vec![c.id.clone()],
                        quantity: 0,
                        manufacturer: String::new(),
                        description: item.description.clone().unwrap_or_default(),
                        part_number: Some(code),
                        catalogue: None,
                        requested: None,
                        status: None,
                        position: item.position.clone(),
                    }
                }
                ComponentKind::Port { .. } => continue,
            };
            let same = |l: &BomLine| {
                l.catalogue == line.catalogue
                    && l.part_number == line.part_number
                    && l.requested == line.requested
                    && l.status == line.status
                    && l.description == line.description
                    && l.position == line.position
            };
            match lines.iter_mut().find(|l| same(l)) {
                Some(l) => l.designators.push(c.id.clone()),
                None => lines.push(line),
            }
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
                catalogue: Some(a.block.clone()),
                requested: None,
                status: None,
                position: None,
            });
        }
        for e in &self.bom_extra {
            lines.push(BomLine {
                designators: Vec::new(),
                quantity: e.quantity as usize,
                manufacturer: String::new(),
                description: e.description.clone().unwrap_or_default(),
                part_number: e.code.clone(),
                catalogue: None,
                requested: None,
                status: None,
                position: e.position.clone(),
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
        "Position",
        "Quantity",
        "Designators",
        "Manufacturer",
        "Description",
        "Part number",
        "Catalogue",
        "Status",
        "Requested",
    ];
    row(&mut out, header.map(String::from));
    for (i, l) in lines.iter().enumerate() {
        row(
            &mut out,
            [
                (i + 1).to_string(),
                l.position.clone().unwrap_or_default(),
                l.quantity.to_string(),
                l.designators.join(" "),
                l.manufacturer.clone(),
                l.description.clone(),
                l.part_number.clone().unwrap_or_default(),
                l.catalogue
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                l.status
                    .map(ValveStatus::words)
                    .unwrap_or_default()
                    .to_string(),
                l.requested.clone().unwrap_or_default(),
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
