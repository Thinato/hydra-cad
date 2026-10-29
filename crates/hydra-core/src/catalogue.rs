//! Catalogue rows the circuit model needs: manufacturers, symbol definitions, valve types.

use serde::{Deserialize, Serialize};

use crate::SymbolDef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manufacturer {
    pub id: String,
    pub name: String,
}

/// A versioned symbol definition. A change creates a new row with a new id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDefRecord {
    pub id: String,
    pub version: u32,
    pub definition: SymbolDef,
}

/// A valve family and spool, e.g. RPE3-06 Z11.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValveType {
    pub id: String,
    pub manufacturer_id: String,
    pub series: String,
    pub code: String,
    pub symbol_def_id: String,
    /// Mounting interface, e.g. `ISO 4401-03`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounting: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
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
}

impl Catalogue {
    pub fn valve_type(&self, id: &str) -> Option<&ValveType> {
        self.valve_types.iter().find(|v| v.id == id)
    }

    pub fn symbol_def(&self, id: &str) -> Option<&SymbolDef> {
        self.symbol_defs
            .iter()
            .find(|s| s.id == id)
            .map(|s| &s.definition)
    }

    /// Symbol of a valve type, if both rows exist.
    pub fn symbol_of(&self, valve_type_id: &str) -> Option<&SymbolDef> {
        self.symbol_def(&self.valve_type(valve_type_id)?.symbol_def_id)
    }

    /// Everything wrong with the catalogue, in plain words. Empty means consistent.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for s in &self.symbol_defs {
            for p in s.definition.problems() {
                problems.push(format!("symbol {}: {p}", s.id));
            }
        }
        for v in &self.valve_types {
            if !self.manufacturers.iter().any(|m| m.id == v.manufacturer_id) {
                problems.push(format!(
                    "valve type {}: unknown manufacturer {}",
                    v.id, v.manufacturer_id
                ));
            }
            if self.symbol_def(&v.symbol_def_id).is_none() {
                problems.push(format!(
                    "valve type {}: unknown symbol {}",
                    v.id, v.symbol_def_id
                ));
            }
        }
        problems
    }
}
