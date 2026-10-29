//! Symbol definitions: what a valve symbol shows, independent of how it is drawn.

use serde::{Deserialize, Serialize};

/// A symbol definition as stored in `symbol_defs.definition`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolDef {
    DirectionalValve(DirectionalValve),
    /// Non-return valve, free flow from `A` to `B`.
    Check {
        #[serde(default)]
        spring: bool,
    },
    /// Pressure relief valve from `P` to `T`.
    Relief {
        #[serde(default)]
        adjustable: bool,
    },
    /// Restriction between `A` and `B`.
    Orifice {
        #[serde(default)]
        adjustable: bool,
    },
}

/// A spool valve: one envelope per switching position, actuators on both ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectionalValve {
    /// Ports in drawing order along their side, e.g. A, B on top and P, T at the bottom.
    pub ports: Vec<Port>,
    /// Envelopes from left to right.
    pub positions: Vec<SpoolPosition>,
    /// Index of the rest position, the envelope the ports connect to.
    pub normal_position: usize,
    #[serde(default)]
    pub left: Vec<Actuator>,
    #[serde(default)]
    pub right: Vec<Actuator>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub side: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Top,
    Bottom,
}

/// What one envelope connects. Ports not mentioned are drawn open-ended.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpoolPosition {
    /// Flow paths drawn as arrows, `[from, to]`.
    #[serde(default)]
    pub flows: Vec<[String; 2]>,
    /// Connections drawn as plain lines, without direction.
    #[serde(default)]
    pub links: Vec<[String; 2]>,
    /// Ports closed in this position.
    #[serde(default)]
    pub blocked: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actuator {
    Solenoid,
    Spring,
    Lever,
    Pilot,
    Detent,
}

impl SymbolDef {
    /// Port names this symbol exposes, in definition order.
    pub fn ports(&self) -> Vec<&str> {
        match self {
            SymbolDef::DirectionalValve(v) => v.ports.iter().map(|p| p.name.as_str()).collect(),
            SymbolDef::Check { .. } | SymbolDef::Orifice { .. } => vec!["A", "B"],
            SymbolDef::Relief { .. } => vec!["P", "T"],
        }
    }

    /// Everything wrong with this definition, in plain words. Empty means well-formed.
    pub fn problems(&self) -> Vec<String> {
        let SymbolDef::DirectionalValve(v) = self else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        if v.ports.is_empty() {
            problems.push("a directional valve needs at least one port".to_string());
        }
        for (i, port) in v.ports.iter().enumerate() {
            if v.ports[..i].iter().any(|p| p.name == port.name) {
                problems.push(format!("port {} is defined twice", port.name));
            }
        }
        if v.positions.is_empty() {
            problems.push("a directional valve needs at least one spool position".to_string());
        } else if v.normal_position >= v.positions.len() {
            problems.push(format!(
                "normal position {} is out of range, the valve has {} positions",
                v.normal_position,
                v.positions.len()
            ));
        }
        let names = self.ports();
        for (i, pos) in v.positions.iter().enumerate() {
            let used = pos
                .flows
                .iter()
                .chain(&pos.links)
                .flatten()
                .chain(&pos.blocked);
            for name in used {
                if !names.contains(&name.as_str()) {
                    problems.push(format!(
                        "position {} refers to port {name}, which the valve does not have",
                        i + 1
                    ));
                }
            }
        }
        problems
    }
}
