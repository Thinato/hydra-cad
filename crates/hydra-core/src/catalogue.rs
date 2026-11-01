//! Catalogue rows the circuit model needs: manufacturers, symbol definitions, valve types and
//! manifold blocks.
//!
//! Symbol definitions, valve types and blocks are versioned: a change adds a row with the same key and
//! the next version, and documents point at one exact version through a [`CatalogueRef`].

use std::collections::BTreeMap;
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

/// A manifold block model and the faces valves mount on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub key: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub mount_faces: Vec<MountFace>,
}

/// A block face that takes one valve. Block coordinates: millimetres, Z up.
///
/// A valve model is drawn with its mounting-pattern datum at the origin, its mounting face in
/// the plane z = 0 and its body in +z; [`MountFace::transform`] puts that frame on the face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountFace {
    pub name: String,
    /// Mounting pattern, e.g. `ISO 4401-03`; a valve fits when its `mounting` is the same.
    pub pattern: String,
    /// The pattern's datum point, on the face.
    pub origin: [f64; 3],
    /// Points out of the block.
    pub normal: [f64; 3],
    /// The pattern's x direction, in the face plane.
    pub x_axis: [f64; 3],
}

impl MountFace {
    /// Column-major 4x4 matrix from valve model space to block space, or why the frame is unusable.
    pub fn transform(&self) -> Result<[[f64; 4]; 4], String> {
        let unit = |v: [f64; 3], what: &str| {
            let len = dot(v, v).sqrt();
            if len.is_finite() && len > 1e-9 {
                Ok(v.map(|c| c / len))
            } else {
                Err(format!("face {}: {what} has no direction", self.name))
            }
        };
        let z = unit(self.normal, "normal")?;
        let x = unit(self.x_axis, "x axis")?;
        if dot(x, z).abs() > 1e-3 {
            return Err(format!(
                "face {}: x axis is not in the face plane (not perpendicular to the normal)",
                self.name
            ));
        }
        if !self.origin.iter().all(|c| c.is_finite()) {
            return Err(format!("face {}: origin is not a point", self.name));
        }
        // Remove the small tilt the tolerance above allows, so the frame has no shear.
        let x = unit([0, 1, 2].map(|k| x[k] - dot(x, z) * z[k]), "x axis")?;
        let y = cross(z, x);
        let [o0, o1, o2] = self.origin;
        Ok([
            [x[0], x[1], x[2], 0.0],
            [y[0], y[1], y[2], 0.0],
            [z[0], z[1], z[2], 0.0],
            [o0, o1, o2, 1.0],
        ])
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl Block {
    pub fn reference(&self) -> CatalogueRef {
        CatalogueRef::new(&self.key, self.version)
    }

    pub fn face(&self, name: &str) -> Option<&MountFace> {
        self.mount_faces.iter().find(|f| f.name == name)
    }
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

    /// Ratings from `params`; values that aren't numbers read as unknown.
    pub fn ratings(&self) -> Ratings {
        Ratings::from_json(&self.params)
    }

    /// A screw-in cartridge: `mounting` names a cavity, not a subplate pattern.
    pub fn is_cartridge(&self) -> bool {
        self.params.get("cavity").and_then(|c| c.as_bool()) == Some(true)
    }
}

/// What a valve does. Stand-ins must do the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValveFunction {
    Directional,
    Check,
    PilotOperatedCheck,
    Relief,
    Counterbalance,
    FlowControl,
    PressureReducing,
    Sequence,
    /// Anything else; never matches another valve.
    Other,
}

impl ValveFunction {
    pub const ALL: [ValveFunction; 9] = [
        ValveFunction::Directional,
        ValveFunction::Check,
        ValveFunction::PilotOperatedCheck,
        ValveFunction::Relief,
        ValveFunction::Counterbalance,
        ValveFunction::FlowControl,
        ValveFunction::PressureReducing,
        ValveFunction::Sequence,
        ValveFunction::Other,
    ];

    /// The name used in JSON, e.g. `pilot_operated_check`.
    pub fn name(self) -> &'static str {
        match self {
            ValveFunction::Directional => "directional",
            ValveFunction::Check => "check",
            ValveFunction::PilotOperatedCheck => "pilot_operated_check",
            ValveFunction::Relief => "relief",
            ValveFunction::Counterbalance => "counterbalance",
            ValveFunction::FlowControl => "flow_control",
            ValveFunction::PressureReducing => "pressure_reducing",
            ValveFunction::Sequence => "sequence",
            ValveFunction::Other => "other",
        }
    }

    pub fn from_name(name: &str) -> Option<ValveFunction> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// Valves set to a pressure: they are compared by setting range, not maximum pressure.
    pub fn is_pressure_setting(self) -> bool {
        matches!(
            self,
            ValveFunction::Relief
                | ValveFunction::Counterbalance
                | ValveFunction::PressureReducing
                | ValveFunction::Sequence
        )
    }
}

impl fmt::Display for ValveFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Rated limits of a valve type or part, read from `params` or a part's `options`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ratings {
    pub max_pressure_bar: Option<f64>,
    pub max_flow_l_min: Option<f64>,
    /// Range the valve can be set to, `[min, max]`.
    pub setting_range_bar: Option<[f64; 2]>,
}

impl Ratings {
    /// Keys under which ratings are stored.
    pub const KEYS: [&'static str; 3] = ["max_pressure_bar", "max_flow_l_min", "setting_range_bar"];

    pub fn from_json(v: &serde_json::Value) -> Ratings {
        let num = |k: &str| v.get(k).and_then(|x| x.as_f64()).filter(|x| x.is_finite());
        let range = v.get("setting_range_bar").and_then(|r| {
            let r = r.as_array()?;
            match r.as_slice() {
                [a, b] => Some([a.as_f64()?, b.as_f64()?]),
                _ => None,
            }
        });
        Ratings {
            max_pressure_bar: num("max_pressure_bar"),
            max_flow_l_min: num("max_flow_l_min"),
            setting_range_bar: range,
        }
    }

    /// `self` with each value `over` knows replaced by it.
    pub fn overridden_by(self, over: Ratings) -> Ratings {
        Ratings {
            max_pressure_bar: over.max_pressure_bar.or(self.max_pressure_bar),
            max_flow_l_min: over.max_flow_l_min.or(self.max_flow_l_min),
            setting_range_bar: over.setting_range_bar.or(self.setting_range_bar),
        }
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
    #[serde(default)]
    pub blocks: Vec<Block>,
}

impl Catalogue {
    pub fn valve_type(&self, r: &CatalogueRef) -> Option<&ValveType> {
        self.valve_types
            .iter()
            .find(|v| v.key == r.key && v.version == r.version)
    }

    pub fn block(&self, r: &CatalogueRef) -> Option<&Block> {
        self.blocks
            .iter()
            .find(|b| b.key == r.key && b.version == r.version)
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

    /// What a valve type does: `params.function`, or else what its symbol shows.
    pub fn function_of(&self, valve_type: &CatalogueRef) -> Option<ValveFunction> {
        let v = self.valve_type(valve_type)?;
        v.params
            .get("function")
            .and_then(|f| f.as_str())
            .and_then(ValveFunction::from_name)
            .or_else(|| self.symbol_of(valve_type)?.function())
    }

    /// Ratings of a valve type, with those a part states in its options taking precedence.
    pub fn ratings_of(&self, valve_type: &CatalogueRef, part: Option<&str>) -> Ratings {
        let base = self
            .valve_type(valve_type)
            .map(ValveType::ratings)
            .unwrap_or_default();
        let part = part.and_then(|n| self.parts_of(valve_type).find(|p| p.part_number == n));
        match part {
            Some(p) => base.overridden_by(Ratings::from_json(&p.options)),
            None => base,
        }
    }

    /// The highest version of every valve type key.
    pub fn latest_valve_types(&self) -> BTreeMap<&str, &ValveType> {
        let mut latest: BTreeMap<&str, &ValveType> = BTreeMap::new();
        for v in &self.valve_types {
            let slot = latest.entry(v.key.as_str()).or_insert(v);
            if v.version > slot.version {
                *slot = v;
            }
        }
        latest
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
            )
            .chain(self.blocks.iter().map(|b| ("block", b.key.as_str())));
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
        for (i, b) in self.blocks.iter().enumerate() {
            let r = b.reference();
            if self.blocks[..i].iter().any(|o| o.reference() == r) {
                problems.push(format!("block {r} is defined twice"));
            }
            for (j, f) in b.mount_faces.iter().enumerate() {
                if f.name.trim().is_empty() {
                    problems.push(format!("block {r}: a mount face has no name"));
                } else if b.mount_faces[..j].iter().any(|o| o.name == f.name) {
                    problems.push(format!("block {r}: face {} is defined twice", f.name));
                }
                if f.pattern.trim().is_empty() {
                    problems.push(format!(
                        "block {r}: face {} has no mounting pattern",
                        f.name
                    ));
                }
                if let Err(e) = f.transform() {
                    problems.push(format!("block {r}: {e}"));
                }
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
