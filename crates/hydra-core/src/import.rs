//! Checks on a circuit the AI assistant read from a drawing, before it is saved.

use crate::{Catalogue, Circuit, ComponentKind, MAX_PROPOSAL_NOTES, Severity, SymbolDef};

/// Most components an imported circuit may have; more is not one drawing.
pub const MAX_IMPORT_COMPONENTS: usize = 500;
/// Most extra bill-of-materials lines an imported circuit may have.
pub const MAX_IMPORT_EXTRA_LINES: usize = 200;
/// Longest circuit name, in characters.
pub const MAX_CIRCUIT_NAME: usize = 200;
/// Longest code, description, label or position, in characters.
pub const MAX_IMPORT_TEXT: usize = 200;
/// Farthest a part may sit from the sheet's origin, in millimetres: well beyond an A0 sheet.
pub const MAX_SHEET_MM: f32 = 10_000.0;
/// Most wires one net may be drawn with; a drawing's busiest line has a few dozen branches.
pub const MAX_WIRES_PER_NET: usize = 100;
/// Most points one wire may have.
pub const MAX_WIRE_POINTS: usize = 100;

impl Circuit {
    /// Where a valve's valve type lists the requested part, use it: the assistant often names the
    /// type and the code but not the part.
    pub fn choose_requested_parts(&mut self, catalogue: &Catalogue) {
        for c in &mut self.components {
            let ComponentKind::Valve(v) = &mut c.kind else {
                continue;
            };
            if v.part.is_some() {
                continue;
            }
            let (Some(t), Some(r)) = (&v.valve_type, &v.requested) else {
                continue;
            };
            if catalogue.parts_of(t).any(|p| p.part_number == r.code) {
                v.part = Some(r.code.clone());
            }
        }
    }

    /// Everything that stops an imported circuit from being saved, in plain words for whoever
    /// read the drawing. Warnings (unconnected ports, missing valves) are not among them.
    pub fn import_problems(&self, catalogue: &Catalogue) -> Vec<String> {
        let mut problems = Vec::new();
        let name = self.name.trim();
        if name != self.name {
            problems.push("the name has spaces at the start or end".into());
        }
        if name.is_empty() {
            problems.push("the circuit needs a name, e.g. the drawing number".into());
        } else if name.chars().count() > MAX_CIRCUIT_NAME {
            problems.push(format!(
                "the name is longer than {MAX_CIRCUIT_NAME} characters"
            ));
        }
        if self.components.len() > MAX_IMPORT_COMPONENTS {
            problems.push(format!(
                "{} components is more than one drawing holds ({MAX_IMPORT_COMPONENTS})",
                self.components.len()
            ));
        }
        if self.bom_extra.len() > MAX_IMPORT_EXTRA_LINES {
            problems.push(format!(
                "{} extra bill-of-materials lines is more than {MAX_IMPORT_EXTRA_LINES}",
                self.bom_extra.len()
            ));
        }
        problems.extend(
            self.validate(catalogue)
                .into_iter()
                .filter(|i| i.severity() == Severity::Error)
                .map(|i| i.to_string()),
        );
        let code = |what: &str, code: &str, problems: &mut Vec<String>| {
            if code.trim().is_empty() {
                problems.push(format!("{what} has an empty code"));
            } else if code.trim() != code {
                problems.push(format!(
                    "{what}: code {code:?} has spaces at the start or end"
                ));
            }
        };
        let long = |what: &str, field: &str, text: &str, problems: &mut Vec<String>| {
            if text.chars().count() > MAX_IMPORT_TEXT {
                problems.push(format!(
                    "{what}: {field} is longer than {MAX_IMPORT_TEXT} characters"
                ));
            }
        };
        for c in &self.components {
            match self.layout.components.get(&c.id) {
                None => problems.push(format!("{} has no place on the sheet (layout)", c.id)),
                Some(p) if p.x.abs() > MAX_SHEET_MM || p.y.abs() > MAX_SHEET_MM => {
                    problems.push(format!(
                        "{} is placed at {}, {}; keep the layout within {MAX_SHEET_MM} mm",
                        c.id, p.x, p.y
                    ));
                }
                Some(_) => {}
            }
            match &c.kind {
                ComponentKind::Valve(v) => {
                    let what = format!("valve {}", c.id);
                    if let Some(r) = &v.requested {
                        code(&what, &r.code, &mut problems);
                        long(&what, "code", &r.code, &mut problems);
                        long(&what, "description", &r.description, &mut problems);
                        for (field, text) in [("key", &r.key), ("mounting", &r.mounting)] {
                            long(&what, field, text.as_deref().unwrap_or(""), &mut problems);
                        }
                    } else if v.stand_in {
                        problems.push(format!(
                            "{what} is marked as a stand-in, but for what? Give requested"
                        ));
                    }
                    if v.valve_type.is_none() {
                        let r = v.requested.as_ref();
                        if r.is_none() {
                            problems.push(format!(
                                "{what} uses no catalogue valve type, so it needs requested \
                                 with the code as printed"
                            ));
                        }
                        if r.is_some_and(|r| r.function.is_none()) {
                            problems.push(format!(
                                "{what} is missing from the catalogue: give requested.function"
                            ));
                        }
                        if r.is_some_and(|r| {
                            r.mounting.as_deref().is_none_or(|m| m.trim().is_empty())
                        }) {
                            problems.push(format!(
                                "{what} is missing from the catalogue: give requested.mounting, \
                                 the subplate pattern or cartridge cavity"
                            ));
                        }
                    }
                    if let Some(d) = &v.display {
                        for p in d.problems() {
                            problems.push(format!("{what} display: {p}"));
                        }
                    }
                }
                ComponentKind::Item(item) => {
                    let what = format!("item {}", c.id);
                    if item.symbol.is_valve() && !matches!(item.symbol, SymbolDef::Orifice { .. }) {
                        problems.push(format!(
                            "{what} is drawn as a valve; valves go in as kind \"valve\" (only a \
                             fixed orifice can be an item)"
                        ));
                    }
                    for p in item.symbol.problems() {
                        problems.push(format!("{what}: {p}"));
                    }
                    if let Some(c) = &item.code {
                        code(&what, c, &mut problems);
                    }
                    for (field, text) in [
                        ("code", &item.code),
                        ("label", &item.label),
                        ("description", &item.description),
                        ("position", &item.position),
                    ] {
                        long(&what, field, text.as_deref().unwrap_or(""), &mut problems);
                    }
                }
                ComponentKind::Port { .. } => {}
            }
        }
        for (net, wires) in &self.layout.wires {
            if !self.nets.iter().any(|n| n.id == *net) {
                problems.push(format!(
                    "layout.wires names net {net:?}, which is not in nets; key each net's wires by \
                     its id"
                ));
                continue;
            }
            if wires.len() > MAX_WIRES_PER_NET {
                problems.push(format!(
                    "net {net} is drawn with {} wires, more than {MAX_WIRES_PER_NET}; join wires \
                     that run on from one another",
                    wires.len()
                ));
                continue;
            }
            for (i, w) in wires.iter().enumerate() {
                let what = format!("wire {} of net {net}", i + 1);
                let far = |p: &[f32; 2]| p.iter().any(|v| !v.is_finite() || v.abs() > MAX_SHEET_MM);
                if w.len() < 2 {
                    problems.push(format!(
                        "{what} has {} point(s); a wire runs between at least 2",
                        w.len()
                    ));
                } else if w.len() > MAX_WIRE_POINTS {
                    problems.push(format!(
                        "{what} has {} points, more than {MAX_WIRE_POINTS}; split it into wires \
                         that meet",
                        w.len()
                    ));
                } else if let Some([x, y]) = w.iter().find(|p| far(p)) {
                    problems.push(format!(
                        "{what} has point {x}, {y}; keep the layout within {MAX_SHEET_MM} mm"
                    ));
                } else if let Some(s) = w
                    .windows(2)
                    .find(|s| s[0][0] != s[1][0] && s[0][1] != s[1][1])
                {
                    let ([x0, y0], [x1, y1]) = (s[0], s[1]);
                    problems.push(format!(
                        "{what} runs diagonally from {x0}, {y0} to {x1}, {y1}; wires run \
                         horizontally or vertically, so add the corner point between them"
                    ));
                }
            }
        }
        for (i, e) in self.bom_extra.iter().enumerate() {
            let what = format!("extra line {}", i + 1);
            if e.quantity < 1 {
                problems.push(format!("{what} has quantity 0"));
            }
            match (&e.code, &e.description) {
                (None, None) => problems.push(format!("{what} needs a code or a description")),
                (Some(c), _) => code(&what, c, &mut problems),
                _ => {}
            }
            for (field, text) in [
                ("code", &e.code),
                ("description", &e.description),
                ("position", &e.position),
            ] {
                long(&what, field, text.as_deref().unwrap_or(""), &mut problems);
            }
        }
        if let Some(import) = &self.import {
            let len: usize = import.notes.iter().map(|n| n.chars().count()).sum();
            if len > MAX_PROPOSAL_NOTES {
                problems.push(format!(
                    "the notes are longer than {MAX_PROPOSAL_NOTES} characters"
                ));
            }
        }
        problems
    }
}
