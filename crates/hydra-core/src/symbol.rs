//! Symbol definitions: what a valve symbol shows, independent of how it is drawn.

use serde::{Deserialize, Serialize};

use crate::ValveFunction;

/// A symbol definition as stored in `symbol_defs.definition`.
///
/// Valve kinds ([`SymbolDef::is_valve`]) can be catalogue symbols; the others draw generic
/// parts of a circuit such as test points and motors.
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
        /// Piloted from another line through port `X`, instead of from `P`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pilot_port: bool,
    },
    /// Restriction between `A` and `B`.
    Orifice {
        #[serde(default)]
        adjustable: bool,
    },
    /// Flow control from `In` to `Out`; with `bypass`, excess flow leaves through `Bypass`.
    FlowControl {
        #[serde(default)]
        actuator: FlowActuator,
        /// Pressure compensated: the flow stays set whatever the load.
        #[serde(default)]
        compensated: bool,
        #[serde(default)]
        bypass: bool,
        /// A second pilot, port `X`, on the compensator.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pilot_port: bool,
    },
    /// Check valve from `A` to `B` that pilot pressure on `X` opens backwards.
    PilotCheck {
        #[serde(default)]
        spring: bool,
    },
    /// Counterbalance (overcenter) valve: free flow `V` to `C`, load held on `C` until pilot
    /// pressure on `X` opens it.
    Counterbalance,
    /// Gauge connection, port `M`.
    TestPoint,
    /// Hydraulic motor between `A` and `B`, with a case drain `L` when `drain`.
    HydraulicMotor {
        #[serde(default)]
        bidirectional: bool,
        #[serde(default)]
        variable: bool,
        #[serde(default)]
        drain: bool,
    },
    /// Pump from suction `S` to pressure `P`.
    Pump {
        #[serde(default)]
        variable: bool,
    },
    /// Labelled box with the given ports, for anything without a symbol of its own.
    Box {
        #[serde(default)]
        label: String,
        ports: Vec<String>,
    },
}

/// How a flow control is set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowActuator {
    #[default]
    Manual,
    Proportional,
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
            SymbolDef::Relief { pilot_port, .. } => {
                let mut ports = vec!["P", "T"];
                if *pilot_port {
                    ports.push("X");
                }
                ports
            }
            SymbolDef::FlowControl {
                bypass, pilot_port, ..
            } => {
                let mut ports = vec!["In", "Out"];
                if *bypass {
                    ports.push("Bypass");
                }
                if *pilot_port {
                    ports.push("X");
                }
                ports
            }
            SymbolDef::PilotCheck { .. } => vec!["A", "B", "X"],
            SymbolDef::Counterbalance => vec!["V", "C", "X"],
            SymbolDef::TestPoint => vec!["M"],
            SymbolDef::HydraulicMotor { drain, .. } => {
                let mut ports = vec!["A", "B"];
                if *drain {
                    ports.push("L");
                }
                ports
            }
            SymbolDef::Pump { .. } => vec!["P", "S"],
            SymbolDef::Box { ports, .. } => ports.iter().map(String::as_str).collect(),
        }
    }

    /// A valve kind, which may be a catalogue symbol. The others are generic circuit parts;
    /// a fixed orifice is both.
    pub fn is_valve(&self) -> bool {
        matches!(
            self,
            SymbolDef::DirectionalValve(_)
                | SymbolDef::Check { .. }
                | SymbolDef::Relief { .. }
                | SymbolDef::Orifice { .. }
                | SymbolDef::FlowControl { .. }
                | SymbolDef::PilotCheck { .. }
                | SymbolDef::Counterbalance
        )
    }

    /// What a valve with this symbol does, when the symbol alone says so.
    pub fn function(&self) -> Option<ValveFunction> {
        Some(match self {
            SymbolDef::DirectionalValve(_) => ValveFunction::Directional,
            SymbolDef::Check { .. } => ValveFunction::Check,
            SymbolDef::PilotCheck { .. } => ValveFunction::PilotOperatedCheck,
            SymbolDef::Relief { .. } => ValveFunction::Relief,
            SymbolDef::Counterbalance => ValveFunction::Counterbalance,
            SymbolDef::FlowControl { .. } | SymbolDef::Orifice { .. } => ValveFunction::FlowControl,
            _ => return None,
        })
    }

    /// Everything wrong with this definition, in plain words. Empty means well-formed.
    pub fn problems(&self) -> Vec<String> {
        match self {
            SymbolDef::DirectionalValve(v) => v.problems(&self.ports()),
            SymbolDef::Box { label, ports } => {
                let mut problems = Vec::new();
                if ports.is_empty() || ports.len() > 8 {
                    problems.push(format!("a box needs 1 to 8 ports, not {}", ports.len()));
                }
                port_name_problems(ports.iter().map(String::as_str), &mut problems);
                if label.chars().count() > 40 {
                    problems.push("a box label must be at most 40 characters".to_string());
                }
                problems
            }
            SymbolDef::FlowControl {
                compensated: false,
                pilot_port: true,
                ..
            } => vec![
                "a flow control's pilot port X works its pressure compensator, so it needs \
                 compensated: true"
                    .to_string(),
            ],
            _ => Vec::new(),
        }
    }
}

/// Duplicate and badly formed port names.
fn port_name_problems<'a>(
    names: impl Iterator<Item = &'a str> + Clone,
    problems: &mut Vec<String>,
) {
    for (i, name) in names.clone().enumerate() {
        if names.clone().take(i).any(|n| n == name) {
            problems.push(format!("port {name} is defined twice"));
        }
        if name.trim().is_empty() || name.trim() != name || name.chars().count() > 8 {
            problems.push(format!(
                "port name {name:?} must be 1 to 8 characters without spaces at the ends"
            ));
        }
    }
}

impl DirectionalValve {
    fn problems(&self, names: &[&str]) -> Vec<String> {
        let mut problems = Vec::new();
        if self.ports.is_empty() {
            problems.push("a directional valve needs at least one port".to_string());
        }
        // Real valves stay well inside these; beyond them the drawing is unreadable.
        for (what, n, max) in [
            ("ports", self.ports.len(), 8),
            ("positions", self.positions.len(), 6),
            ("actuators on the left", self.left.len(), 4),
            ("actuators on the right", self.right.len(), 4),
        ] {
            if n > max {
                problems.push(format!("{n} {what} is more than a symbol can show ({max})"));
            }
        }
        port_name_problems(names.iter().copied(), &mut problems);
        if self.positions.is_empty() {
            problems.push("a directional valve needs at least one spool position".to_string());
        } else if self.normal_position >= self.positions.len() {
            problems.push(format!(
                "normal position {} is out of range, the valve has {} positions",
                self.normal_position,
                self.positions.len()
            ));
        }
        for (i, pos) in self.positions.iter().enumerate() {
            for [a, b] in pos.flows.iter().chain(&pos.links) {
                if a == b {
                    problems.push(format!("position {} connects port {a} to itself", i + 1));
                }
            }
            for name in &pos.blocked {
                if pos
                    .flows
                    .iter()
                    .chain(&pos.links)
                    .flatten()
                    .any(|p| p == name)
                {
                    problems.push(format!(
                        "position {} both blocks and connects port {name}",
                        i + 1
                    ));
                }
            }
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
