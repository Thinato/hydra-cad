//! Catalogue entries drafted from a datasheet, usually by an AI assistant, waiting for an
//! engineer's review. Only approval turns a proposal into catalogue rows.

use serde::{Deserialize, Serialize};

use crate::{Catalogue, CatalogueRef, Part, SymbolDefRecord, ValveType, key_problem};

/// Most parts one proposal may list; more is not a datasheet, and checks are quadratic.
pub const MAX_PROPOSAL_PARTS: usize = 500;
/// Most characters of a proposal's notes.
pub const MAX_PROPOSAL_NOTES: usize = 20_000;

/// A valve type as it should be saved, with a new symbol definition if no existing one fits.
///
/// Approving saves `valve_type` as `key@(version + 1)`: like an edit in the app, `version`
/// names the version the proposal updates, or 0 for a new key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub valve_type: ValveType,
    /// Saved first, when the valve type needs a symbol the catalogue doesn't have. Its version
    /// is the next of its key (1 for a new key), and `valve_type.symbol` must point at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_symbol: Option<SymbolDefRecord>,
    /// Part numbers of the new version. Parts of the updated version are kept too, unless
    /// listed here again with new options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartDraft>,
    /// Where values came from in the datasheet, and anything the reviewer should check.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// An orderable part number with its ordering-code options (voltage, connector, seals...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartDraft {
    pub part_number: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl Proposal {
    /// The valve type version approval creates.
    pub fn target(&self) -> CatalogueRef {
        CatalogueRef::new(
            &self.valve_type.key,
            self.valve_type.version.saturating_add(1),
        )
    }

    /// The catalogue as it would be after approval: the new symbol, the new valve type version
    /// and its parts (kept ones and proposed ones).
    pub fn apply(&self, catalogue: &Catalogue) -> Catalogue {
        let mut after = catalogue.clone();
        let target = self.target();
        let base = CatalogueRef::new(&self.valve_type.key, self.valve_type.version);
        after.symbol_defs.extend(self.new_symbol.clone());
        after.valve_types.push(ValveType {
            version: target.version,
            ..self.valve_type.clone()
        });
        let kept: Vec<Part> = catalogue
            .parts_of(&base)
            .filter(|p| !self.parts.iter().any(|d| d.part_number == p.part_number))
            .map(|p| Part {
                valve_type: target.clone(),
                ..p.clone()
            })
            .collect();
        after.parts.extend(kept);
        after.parts.extend(self.parts.iter().map(|d| Part {
            part_number: d.part_number.clone(),
            valve_type: target.clone(),
            options: d.options.clone(),
        }));
        after
    }

    /// Everything that stops approval, in plain words for whoever drafted it. Empty means the
    /// proposal can be approved against `catalogue` as it is now.
    pub fn problems(&self, catalogue: &Catalogue) -> Vec<String> {
        let mut problems = Vec::new();
        let v = &self.valve_type;
        if let Some(p) = key_problem(&v.key) {
            problems.push(format!("valve type key {:?} {p}", v.key));
            return problems;
        }
        let latest = |versions: &mut dyn Iterator<Item = u32>| versions.max().unwrap_or(0);
        let current = latest(
            &mut catalogue
                .valve_types
                .iter()
                .filter(|o| o.key == v.key)
                .map(|o| o.version),
        );
        if v.version != current {
            problems.push(match (current, v.version) {
                (0, _) => format!(
                    "valve type {} doesn't exist yet; use version 0 to propose a new one",
                    v.key
                ),
                (n, 0) => format!(
                    "valve type {} already exists at version {n}; use version {n} to update it",
                    v.key
                ),
                (n, _) => format!(
                    "valve type {} is at version {n}; base the proposal on version {n}",
                    v.key
                ),
            });
        }
        if v.series.trim().is_empty() || v.code.trim().is_empty() {
            problems.push("the valve type needs a series and a code".into());
        }
        for (what, text) in [("series", &v.series), ("code", &v.code)] {
            if !text.trim().is_empty() && text.trim() != text {
                problems.push(format!(
                    "the {what} {text:?} has spaces at the start or end"
                ));
            }
        }
        if self.parts.len() > MAX_PROPOSAL_PARTS {
            problems.push(format!(
                "{} parts is more than one proposal takes ({MAX_PROPOSAL_PARTS})",
                self.parts.len()
            ));
            return problems;
        }
        if self.notes.chars().count() > MAX_PROPOSAL_NOTES {
            problems.push(format!(
                "the notes are longer than {MAX_PROPOSAL_NOTES} characters"
            ));
        }
        // The engineer chose the manufacturer when creating the valve type.
        if let Some(base) = catalogue.valve_type(&CatalogueRef::new(&v.key, v.version))
            && base.manufacturer != v.manufacturer
        {
            problems.push(format!(
                "valve type {} is made by {}; a proposal can't change the manufacturer",
                v.key, base.manufacturer
            ));
        }
        if v.mounting.as_deref().is_some_and(|m| m.trim().is_empty()) {
            problems.push(
                "mounting is empty; leave it out if the valve has no subplate pattern".into(),
            );
        }
        if !catalogue
            .manufacturers
            .iter()
            .any(|m| m.key == v.manufacturer)
        {
            let known: Vec<_> = catalogue
                .manufacturers
                .iter()
                .map(|m| m.key.as_str())
                .collect();
            problems.push(format!(
                "unknown manufacturer {:?}; use one of these keys: {}",
                v.manufacturer,
                known.join(", ")
            ));
        }
        match (&v.symbol, &self.new_symbol) {
            (Some(s), None) if catalogue.symbol_def(s).is_none() => problems.push(format!(
                "unknown symbol {s}; use an existing symbol definition or define it in new_symbol"
            )),
            (None, _) => problems.push(
                "the valve type needs a symbol: an existing symbol definition, or new_symbol".into(),
            ),
            (Some(s), Some(new)) if *s != new.reference() => problems.push(format!(
                "new_symbol is {} but the valve type uses {s}; point the valve type at the new symbol",
                new.reference()
            )),
            _ => {}
        }
        if let Some(new) = &self.new_symbol {
            // A new key, so an existing symbol other valve types use never changes meaning.
            let existing = latest(
                &mut catalogue
                    .symbol_defs
                    .iter()
                    .filter(|o| o.key == new.key)
                    .map(|o| o.version),
            );
            if existing > 0 {
                problems.push(format!(
                    "symbol key {} already exists; reuse {}@{existing} as the valve type's symbol, \
                     or give new_symbol a new key",
                    new.key, new.key
                ));
            } else if new.version != 1 {
                problems.push(format!("new_symbol {} must be version 1", new.reference()));
            }
            for p in new.definition.problems() {
                problems.push(format!("new_symbol {}: {p}", new.reference()));
            }
        }
        for (i, p) in self.parts.iter().enumerate() {
            if self.parts[..i]
                .iter()
                .any(|o| o.part_number == p.part_number)
            {
                problems.push(format!("part {} is listed twice", p.part_number));
            }
            if p.part_number.trim() != p.part_number {
                problems.push(format!(
                    "part number {:?} has spaces at the start or end",
                    p.part_number
                ));
            }
            // One orderable part belongs to one valve type.
            let owner = catalogue.parts.iter().find(|o| {
                o.part_number == p.part_number
                    && o.valve_type.key != v.key
                    && !catalogue
                        .valve_types
                        .iter()
                        .any(|t| t.key == o.valve_type.key && t.version > o.valve_type.version)
            });
            if let Some(o) = owner {
                problems.push(format!(
                    "part {} already belongs to valve type {}",
                    p.part_number, o.valve_type
                ));
            }
        }
        if !problems.is_empty() {
            return problems;
        }
        // Whatever else is wrong shows up as a new problem of the catalogue after approval.
        let before = catalogue.problems();
        problems.extend(
            self.apply(catalogue)
                .problems()
                .into_iter()
                .filter(|p| !before.contains(p)),
        );
        problems
    }
}
