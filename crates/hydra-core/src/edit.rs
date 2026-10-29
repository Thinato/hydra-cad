//! Edits to a circuit. The editor changes a circuit only through [`Command`]s, which gives
//! undo/redo and a readable log of what changed.

use crate::{Assembly, CatalogueRef, Circuit, Component, ComponentKind, Net, Placement, PortRef};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Add {
        component: Component,
        placement: Placement,
    },
    /// Remove a component and its connections.
    Remove { id: String },
    /// Move and/or rotate a component.
    Place { id: String, placement: Placement },
    /// Put two ports on the same net, merging nets if both are connected already.
    Connect { a: PortRef, b: PortRef },
    /// Take a port off its net.
    Disconnect { port: PortRef },
    /// Rename an external connection such as a manifold port.
    SetLabel { id: String, label: String },
    /// Rename the circuit.
    Rename { name: String },
    /// Build the circuit on a block, or on none. Mounts are kept only for a new version of the
    /// same block.
    SetBlock { block: Option<CatalogueRef> },
    /// Mount a valve on a face of the circuit's block, taking off any valve already there, or
    /// take it off with `None`.
    Mount {
        component: String,
        face: Option<String>,
    },
}

impl Command {
    /// Short description for menus and logs, e.g. "Move V1".
    pub fn describe(&self) -> String {
        match self {
            Command::Add { component, .. } => format!("Add {}", component.id),
            Command::Remove { id } => format!("Delete {id}"),
            Command::Place { id, .. } => format!("Move {id}"),
            Command::Connect { a, b } => format!("Connect {a} to {b}"),
            Command::Disconnect { port } => format!("Disconnect {port}"),
            Command::SetLabel { id, label } => format!("Rename {id} to {label}"),
            Command::Rename { name } => format!("Rename circuit to {name}"),
            Command::SetBlock { block: Some(b) } => format!("Use block {b}"),
            Command::SetBlock { block: None } => "Remove the block".into(),
            Command::Mount {
                component,
                face: Some(f),
            } => format!("Mount {component} on {f}"),
            Command::Mount {
                component,
                face: None,
            } => format!("Unmount {component}"),
        }
    }

    /// Apply to `circuit`, or explain why not. A refused command leaves the circuit unchanged.
    pub fn apply(&self, circuit: &mut Circuit) -> Result<(), String> {
        match self {
            Command::Add {
                component,
                placement,
            } => {
                if circuit.component(&component.id).is_some() {
                    return Err(format!("{} already exists", component.id));
                }
                circuit.components.push(component.clone());
                circuit
                    .layout
                    .components
                    .insert(component.id.clone(), *placement);
            }
            Command::Remove { id } => {
                exists(circuit, id)?;
                circuit.components.retain(|c| &c.id != id);
                circuit.layout.components.remove(id);
                if let Some(a) = &mut circuit.assembly {
                    a.mounts.remove(id);
                }
                let touched = nets_of(circuit, id);
                for net in &mut circuit.nets {
                    net.ports.retain(|p| &p.component != id);
                }
                forget(circuit, &touched);
            }
            Command::Place { id, placement } => {
                exists(circuit, id)?;
                circuit.layout.components.insert(id.clone(), *placement);
                // Hand-drawn wires no longer meet moved ports; draw these nets automatically again.
                let touched = nets_of(circuit, id);
                forget(circuit, &touched);
            }
            Command::Connect { a, b } => {
                if a == b {
                    return Err(format!("can't connect {a} to itself"));
                }
                exists(circuit, &a.component)?;
                exists(circuit, &b.component)?;
                let net_of = |p: &PortRef| circuit.nets.iter().position(|n| n.ports.contains(p));
                match (net_of(a), net_of(b)) {
                    (Some(i), Some(j)) if i == j => {
                        return Err(format!("{a} and {b} are already connected"));
                    }
                    (Some(i), Some(j)) => {
                        // Merge the second net into the first.
                        let merged = circuit.nets.remove(j);
                        let i = if j < i { i - 1 } else { i };
                        circuit.nets[i].ports.extend(merged.ports);
                        if circuit.nets[i].name.is_none() {
                            circuit.nets[i].name = merged.name;
                        }
                        let ids = [circuit.nets[i].id.clone(), merged.id];
                        circuit.layout.wires.retain(|k, _| !ids.contains(k));
                    }
                    (Some(i), None) | (None, Some(i)) => {
                        let loose = if circuit.nets[i].ports.contains(a) {
                            b
                        } else {
                            a
                        };
                        circuit.nets[i].ports.push(loose.clone());
                        let id = circuit.nets[i].id.clone();
                        circuit.layout.wires.remove(&id);
                    }
                    (None, None) => {
                        let id = next_id(circuit.nets.iter().map(|n| n.id.as_str()), "n");
                        circuit.nets.push(Net {
                            id,
                            name: None,
                            ports: vec![a.clone(), b.clone()],
                        });
                    }
                }
            }
            Command::Disconnect { port } => {
                let net = circuit
                    .nets
                    .iter_mut()
                    .find(|n| n.ports.contains(port))
                    .ok_or_else(|| format!("{port} is not connected"))?;
                net.ports.retain(|p| p != port);
                let id = net.id.clone();
                forget(circuit, &[id]);
            }
            Command::SetLabel { id, label } => {
                let c = circuit
                    .components
                    .iter_mut()
                    .find(|c| &c.id == id)
                    .ok_or_else(|| format!("{id} is not in the circuit"))?;
                match &mut c.kind {
                    ComponentKind::Port { label: l } => *l = label.clone(),
                    ComponentKind::Valve { .. } => {
                        return Err(format!("{id} is a valve, not a port"));
                    }
                }
            }
            Command::Rename { name } => {
                if name.trim().is_empty() {
                    return Err("a circuit needs a name".into());
                }
                circuit.name = name.trim().to_string();
            }
            Command::SetBlock { block } => {
                let current = circuit.assembly.as_ref().map(|a| &a.block);
                if current == block.as_ref() {
                    return Err("the circuit already uses that block".into());
                }
                let old = circuit.assembly.take();
                circuit.assembly = block.clone().map(|block| Assembly {
                    mounts: old
                        .filter(|o| o.block.key == block.key)
                        .map(|o| o.mounts)
                        .unwrap_or_default(),
                    block,
                });
            }
            Command::Mount { component, face } => {
                let is_valve = circuit
                    .component(component)
                    .map(|c| matches!(c.kind, ComponentKind::Valve { .. }));
                match is_valve {
                    None => return Err(format!("{component} is not in the circuit")),
                    Some(false) => return Err(format!("{component} is not a valve")),
                    Some(true) => {}
                }
                let assembly = circuit
                    .assembly
                    .as_mut()
                    .ok_or("choose a block for the circuit first")?;
                match face {
                    Some(face) => {
                        if assembly.mounts.get(component) == Some(face) {
                            return Err(format!("{component} is already on {face}"));
                        }
                        // A valve already on that face comes off it.
                        assembly.mounts.retain(|_, f| f != face);
                        assembly.mounts.insert(component.clone(), face.clone());
                    }
                    None => {
                        if assembly.mounts.remove(component).is_none() {
                            return Err(format!("{component} is not mounted"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn exists(circuit: &Circuit, id: &str) -> Result<(), String> {
    match circuit.component(id) {
        Some(_) => Ok(()),
        None => Err(format!("{id} is not in the circuit")),
    }
}

/// Ids of the nets a component is on.
fn nets_of(circuit: &Circuit, id: &str) -> Vec<String> {
    circuit
        .nets
        .iter()
        .filter(|n| n.ports.iter().any(|p| p.component == id))
        .map(|n| n.id.clone())
        .collect()
}

/// Drop hand-drawn wires of the given nets, and those of them left with fewer than two ports.
fn forget(circuit: &mut Circuit, nets: &[String]) {
    circuit.layout.wires.retain(|k, _| !nets.contains(k));
    circuit
        .nets
        .retain(|n| !nets.contains(&n.id) || n.ports.len() >= 2);
}

/// First free id of the form `{prefix}{n}`, counting from 1.
pub fn next_id<'a>(taken: impl Iterator<Item = &'a str> + Clone, prefix: &str) -> String {
    (1..)
        .map(|n| format!("{prefix}{n}"))
        .find(|id| !taken.clone().any(|t| t == id))
        .expect("a free id exists")
}

/// A circuit being edited, with undo and redo.
///
/// ponytail: undo keeps a whole-circuit snapshot per edit, which is simple and always correct.
/// Circuits are small; store inverse commands instead if documents ever get large.
#[derive(Debug, Clone)]
pub struct Document {
    pub circuit: Circuit,
    undo: Vec<(String, Circuit)>,
    redo: Vec<(String, Circuit)>,
    saved: Circuit,
}

impl Document {
    pub fn new(circuit: Circuit) -> Self {
        Self {
            saved: circuit.clone(),
            circuit,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn apply(&mut self, command: Command) -> Result<(), String> {
        let before = self.circuit.clone();
        command.apply(&mut self.circuit)?;
        self.undo.push((command.describe(), before));
        self.redo.clear();
        Ok(())
    }

    /// Undo the last edit; returns its description.
    pub fn undo(&mut self) -> Option<String> {
        let (what, before) = self.undo.pop()?;
        let after = std::mem::replace(&mut self.circuit, before);
        self.redo.push((what.clone(), after));
        Some(what)
    }

    /// Redo the last undone edit; returns its description.
    pub fn redo(&mut self) -> Option<String> {
        let (what, after) = self.redo.pop()?;
        let before = std::mem::replace(&mut self.circuit, after);
        self.undo.push((what.clone(), before));
        Some(what)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(w, _)| w.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(w, _)| w.as_str())
    }

    /// The circuit differs from what was last saved or opened.
    pub fn is_dirty(&self) -> bool {
        self.circuit != self.saved
    }

    /// Record that `circuit` was saved. Edits made while the save was in flight stay dirty.
    pub fn mark_saved_as(&mut self, circuit: Circuit) {
        self.saved = circuit;
    }
}
