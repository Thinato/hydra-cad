//! The circuit document: components, nets and layout, saved whole as one JSON revision.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Catalogue, CatalogueRef};

/// Circuit JSON version this build writes. It also reads version 1, which had no assembly.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Circuit {
    pub schema: u32,
    pub name: String,
    pub components: Vec<Component>,
    #[serde(default)]
    pub nets: Vec<Net>,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<Assembly>,
}

/// The manifold block a circuit is built on, and which valve sits on which of its faces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    pub block: CatalogueRef,
    /// Face name per component id.
    #[serde(default)]
    pub mounts: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    /// Designator, unique in the circuit, e.g. `V1`.
    pub id: String,
    #[serde(flatten)]
    pub kind: ComponentKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComponentKind {
    /// One version of a catalogue valve type.
    Valve { valve_type: CatalogueRef },
    /// An external connection such as a manifold port `P` or `A1`. Its single port is [`PORT_PIN`].
    Port { label: String },
}

/// Name of the one port an external-connection component has.
pub const PORT_PIN: &str = "port";

/// Ports that are hydraulically the same line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Net {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub ports: Vec<PortRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PortRef {
    pub component: String,
    pub port: String,
}

/// Where things sit on the sheet. Units are millimetres, x right, y down.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub components: BTreeMap<String, Placement>,
    /// Hand-drawn wires per net id. Nets without an entry are drawn with automatic elbows.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub wires: BTreeMap<String, Vec<Vec<[f32; 2]>>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub rotation: Rotation,
}

/// Clockwise rotation in quarter turns, written in degrees in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl TryFrom<u16> for Rotation {
    type Error = String;
    fn try_from(deg: u16) -> Result<Self, String> {
        match deg {
            0 => Ok(Rotation::R0),
            90 => Ok(Rotation::R90),
            180 => Ok(Rotation::R180),
            270 => Ok(Rotation::R270),
            _ => Err(format!("rotation must be 0, 90, 180 or 270, got {deg}")),
        }
    }
}

impl From<Rotation> for u16 {
    fn from(r: Rotation) -> u16 {
        match r {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a circuit document: {0}")]
    Json(#[from] serde_json::Error),
    #[error("circuit document has no schema version")]
    MissingSchema,
    #[error(
        "circuit schema version {0} is not supported, this build reads versions 1 to {SCHEMA_VERSION}"
    )]
    UnsupportedSchema(u64),
}

impl Circuit {
    /// Parse a circuit document, refusing schema versions this build does not know. Older
    /// versions are upgraded, so the circuit saves as the current version.
    pub fn from_json(json: &str) -> Result<Circuit, LoadError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        match value.get("schema").and_then(|s| s.as_u64()) {
            None => Err(LoadError::MissingSchema),
            Some(v) if !(1..=u64::from(SCHEMA_VERSION)).contains(&v) => {
                Err(LoadError::UnsupportedSchema(v))
            }
            Some(_) => {
                let mut circuit: Circuit = serde_json::from_value(value)?;
                circuit.schema = SCHEMA_VERSION;
                Ok(circuit)
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("circuit serializes")
    }

    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }

    /// Check the circuit against the catalogue.
    pub fn validate(&self, catalogue: &Catalogue) -> Vec<Issue> {
        let mut issues = Vec::new();
        for (i, c) in self.components.iter().enumerate() {
            if self.components[..i].iter().any(|o| o.id == c.id) {
                issues.push(Issue::DuplicateComponent(c.id.clone()));
            }
            if let ComponentKind::Valve { valve_type } = &c.kind {
                if catalogue.valve_type(valve_type).is_none() {
                    issues.push(Issue::UnknownValveType {
                        component: c.id.clone(),
                        valve_type: valve_type.clone(),
                    });
                } else if catalogue.symbol_of(valve_type).is_none() {
                    issues.push(Issue::MissingSymbol {
                        component: c.id.clone(),
                        valve_type: valve_type.clone(),
                    });
                }
            }
        }

        let mut seen: BTreeMap<&PortRef, &str> = BTreeMap::new();
        for (i, net) in self.nets.iter().enumerate() {
            if self.nets[..i].iter().any(|o| o.id == net.id) {
                issues.push(Issue::DuplicateNet(net.id.clone()));
            }
            for p in &net.ports {
                let Some(component) = self.component(&p.component) else {
                    issues.push(Issue::UnknownComponent {
                        net: net.id.clone(),
                        component: p.component.clone(),
                    });
                    continue;
                };
                if let Some(ports) = component.ports(catalogue)
                    && !ports.contains(&p.port.as_str())
                {
                    issues.push(Issue::UnknownPort {
                        net: net.id.clone(),
                        port: p.clone(),
                    });
                }
                // A port listed twice in one net is harmless; only a second net is a short.
                if let Some(first) = seen.insert(p, &net.id)
                    && first != net.id
                {
                    issues.push(Issue::PortOnTwoNets {
                        port: p.clone(),
                        nets: [first.to_string(), net.id.clone()],
                    });
                }
            }
        }

        for c in &self.components {
            for port in c.ports(catalogue).unwrap_or_default() {
                let p = PortRef {
                    component: c.id.clone(),
                    port: port.to_string(),
                };
                if !seen.contains_key(&p) {
                    issues.push(Issue::Unconnected(p));
                }
            }
        }
        if let Some(assembly) = &self.assembly {
            self.validate_assembly(assembly, catalogue, &mut issues);
        }
        issues
    }

    fn validate_assembly(
        &self,
        assembly: &Assembly,
        catalogue: &Catalogue,
        issues: &mut Vec<Issue>,
    ) {
        let block = catalogue.block(&assembly.block);
        if block.is_none() {
            issues.push(Issue::UnknownBlock(assembly.block.clone()));
        }
        let mut holders: BTreeMap<&str, &str> = BTreeMap::new();
        for (component, face) in &assembly.mounts {
            let valve_type = match self.component(component).map(|c| &c.kind) {
                None => {
                    issues.push(Issue::MountedUnknownComponent {
                        component: component.clone(),
                        face: face.clone(),
                    });
                    continue;
                }
                Some(ComponentKind::Port { .. }) => {
                    issues.push(Issue::MountedPort(component.clone()));
                    continue;
                }
                Some(ComponentKind::Valve { valve_type }) => valve_type,
            };
            if let Some(other) = holders.insert(face, component) {
                issues.push(Issue::FaceTaken {
                    face: face.clone(),
                    components: [other.to_string(), component.clone()],
                });
            }
            let Some(block) = block else { continue };
            let Some(mount_face) = block.face(face) else {
                issues.push(Issue::UnknownFace {
                    component: component.clone(),
                    face: face.clone(),
                });
                continue;
            };
            if let Some(v) = catalogue.valve_type(valve_type)
                && v.mounting.as_deref() != Some(mount_face.pattern.as_str())
            {
                issues.push(Issue::PatternMismatch {
                    component: component.clone(),
                    face: face.clone(),
                    face_pattern: mount_face.pattern.clone(),
                    mounting: v.mounting.clone(),
                });
            }
        }
        for c in &self.components {
            if let ComponentKind::Valve { valve_type } = &c.kind
                && catalogue
                    .valve_type(valve_type)
                    .is_some_and(|v| v.mounting.is_some())
                && !assembly.mounts.contains_key(&c.id)
            {
                issues.push(Issue::Unmounted(c.id.clone()));
            }
        }
    }
}

impl Component {
    /// Port names of this component, or `None` if its catalogue rows are missing.
    pub fn ports<'a>(&self, catalogue: &'a Catalogue) -> Option<Vec<&'a str>> {
        match &self.kind {
            ComponentKind::Valve { valve_type } => Some(catalogue.symbol_of(valve_type)?.ports()),
            ComponentKind::Port { .. } => Some(vec![PORT_PIN]),
        }
    }
}

impl fmt::Display for PortRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.component, self.port)
    }
}

/// A validation finding. Display gives the plain-words message.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Issue {
    #[error("component {0} is defined twice")]
    DuplicateComponent(String),
    #[error("net {0} is defined twice")]
    DuplicateNet(String),
    #[error("component {component} uses valve type {valve_type}, which is not in the catalogue")]
    UnknownValveType {
        component: String,
        valve_type: CatalogueRef,
    },
    #[error("component {component}: valve type {valve_type} has no symbol in the catalogue")]
    MissingSymbol {
        component: String,
        valve_type: CatalogueRef,
    },
    #[error("net {net} refers to component {component}, which is not in the circuit")]
    UnknownComponent { net: String, component: String },
    #[error("net {net} refers to port {port}, which that component does not have")]
    UnknownPort { net: String, port: PortRef },
    #[error("port {port} is on two nets, {} and {}", nets[0], nets[1])]
    PortOnTwoNets { port: PortRef, nets: [String; 2] },
    #[error("port {0} is not connected")]
    Unconnected(PortRef),
    #[error("block {0} is not in the catalogue")]
    UnknownBlock(CatalogueRef),
    #[error("face {face} holds component {component}, which is not in the circuit")]
    MountedUnknownComponent { component: String, face: String },
    #[error("{0} is an external connection; only valves mount on the block")]
    MountedPort(String),
    #[error("{component} is mounted on face {face}, which the block does not have")]
    UnknownFace { component: String, face: String },
    #[error("face {face} holds both {} and {}", components[0], components[1])]
    FaceTaken {
        face: String,
        components: [String; 2],
    },
    #[error("{component} {}, but face {face} takes {face_pattern}", match mounting {
        Some(m) => format!("needs a {m} face"),
        None => "is not a subplate valve".into(),
    })]
    PatternMismatch {
        component: String,
        face: String,
        face_pattern: String,
        mounting: Option<String>,
    },
    #[error("{0} is not mounted on the block")]
    Unmounted(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Issue {
    pub fn severity(&self) -> Severity {
        match self {
            Issue::Unconnected(_) | Issue::Unmounted(_) => Severity::Warning,
            _ => Severity::Error,
        }
    }
}
