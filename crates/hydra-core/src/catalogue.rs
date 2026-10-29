//! Catalogue rows the circuit model needs: manufacturers, symbol definitions, valve types.
//!
//! Symbol definitions and valve types are versioned: a change adds a row with the same key and
//! the next version, and documents point at one exact version through a [`CatalogueRef`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::SymbolDef;

/// One version of a versioned catalogue row, written `key@version` in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CatalogueRef {
    pub key: String,
    pub version: u32,
}

impl CatalogueRef {
    pub fn new(key: impl Into<String>, version: u32) -> Self {
        Self {
            key: key.into(),
            version,
        }
    }
}

impl fmt::Display for CatalogueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.key, self.version)
    }
}

impl TryFrom<String> for CatalogueRef {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let parsed = s.rsplit_once('@').and_then(|(key, version)| {
            let version = version.parse().ok().filter(|v| *v > 0)?;
            (!key.is_empty()).then(|| CatalogueRef::new(key, version))
        });
        parsed.ok_or_else(|| format!("catalogue reference must look like key@version, got {s:?}"))
    }
}

impl From<CatalogueRef> for String {
    fn from(r: CatalogueRef) -> String {
        r.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manufacturer {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDefRecord {
    pub key: String,
    pub version: u32,
    pub definition: SymbolDef,
}

/// A valve family and spool, e.g. RPE3-06 Z11.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValveType {
    pub key: String,
    pub version: u32,
    /// Manufacturer key.
    pub manufacturer: String,
    pub series: String,
    pub code: String,
    /// `None` while the entry is a draft waiting for its symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<CatalogueRef>,
    /// Mounting interface, e.g. `ISO 4401-03`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounting: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
}

/// An orderable part number of one valve type version, with its ordering-code options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub part_number: String,
    pub valve_type: CatalogueRef,
    /// Ordering-code options: voltage, connector, seals...
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl SymbolDefRecord {
    pub fn reference(&self) -> CatalogueRef {
        CatalogueRef::new(&self.key, self.version)
    }
}

impl ValveType {
    pub fn reference(&self) -> CatalogueRef {
        CatalogueRef::new(&self.key, self.version)
    }
}

/// The catalogue rows a circuit is checked and drawn against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalogue {
    #[serde(default)]
    pub manufacturers: Vec<Manufacturer>,
    #[serde(default)]
    pub symbol_defs: Vec<SymbolDefRecord>,
    #[serde(default)]
    pub valve_types: Vec<ValveType>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

impl Catalogue {
    pub fn valve_type(&self, r: &CatalogueRef) -> Option<&ValveType> {
        self.valve_types
            .iter()
            .find(|v| v.key == r.key && v.version == r.version)
    }

    pub fn symbol_def(&self, r: &CatalogueRef) -> Option<&SymbolDef> {
        self.symbol_defs
            .iter()
            .find(|s| s.key == r.key && s.version == r.version)
            .map(|s| &s.definition)
    }

    /// Symbol of a valve type, if the valve type has one and both rows exist.
    pub fn symbol_of(&self, valve_type: &CatalogueRef) -> Option<&SymbolDef> {
        self.symbol_def(self.valve_type(valve_type)?.symbol.as_ref()?)
    }

    /// Parts orderable for one valve type version.
    pub fn parts_of<'a>(&'a self, valve_type: &'a CatalogueRef) -> impl Iterator<Item = &'a Part> {
        self.parts
            .iter()
            .filter(move |p| &p.valve_type == valve_type)
    }

    /// Everything wrong with the catalogue, in plain words. Empty means consistent.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let keys = self
            .manufacturers
            .iter()
            .map(|m| ("manufacturer", m.key.as_str()));
        let keys = keys
            .chain(self.symbol_defs.iter().map(|s| ("symbol", s.key.as_str())))
            .chain(
                self.valve_types
                    .iter()
                    .map(|v| ("valve type", v.key.as_str())),
            );
        for (what, key) in keys {
            if let Some(p) = key_problem(key) {
                problems.push(format!("{what} key {key:?} {p}"));
            }
        }
        for (i, m) in self.manufacturers.iter().enumerate() {
            if self.manufacturers[..i].iter().any(|o| o.key == m.key) {
                problems.push(format!("manufacturer {} is defined twice", m.key));
            }
        }
        for (i, s) in self.symbol_defs.iter().enumerate() {
            if self.symbol_defs[..i]
                .iter()
                .any(|o| o.reference() == s.reference())
            {
                problems.push(format!("symbol {} is defined twice", s.reference()));
            }
            for p in s.definition.problems() {
                problems.push(format!("symbol {}: {p}", s.reference()));
            }
        }
        for (i, v) in self.valve_types.iter().enumerate() {
            let r = v.reference();
            if self.valve_types[..i].iter().any(|o| o.reference() == r) {
                problems.push(format!("valve type {r} is defined twice"));
            }
            if !self.manufacturers.iter().any(|m| m.key == v.manufacturer) {
                problems.push(format!(
                    "valve type {r}: unknown manufacturer {}",
                    v.manufacturer
                ));
            }
            if let Some(s) = &v.symbol
                && self.symbol_def(s).is_none()
            {
                problems.push(format!("valve type {r}: unknown symbol {s}"));
            }
        }
        for (i, p) in self.parts.iter().enumerate() {
            if p.part_number.trim().is_empty() {
                problems.push(format!("a part of {} has no part number", p.valve_type));
            } else if self.parts[..i]
                .iter()
                .any(|o| o.part_number == p.part_number && o.valve_type == p.valve_type)
            {
                problems.push(format!(
                    "part {} of {} is defined twice",
                    p.part_number, p.valve_type
                ));
            }
            if self.valve_type(&p.valve_type).is_none() {
                problems.push(format!(
                    "part {}: unknown valve type {}",
                    p.part_number, p.valve_type
                ));
            }
        }
        problems
    }
}

/// Why `key` can't be a catalogue key, or `None` if it can. Keys appear in `key@version`.
pub fn key_problem(key: &str) -> Option<&'static str> {
    if key.is_empty() {
        Some("is empty")
    } else if key.trim() != key {
        Some("has spaces at the start or end")
    } else if key.contains('@') {
        Some("contains '@'")
    } else {
        None
    }
}
