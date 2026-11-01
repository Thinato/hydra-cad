//! Edits to a circuit. The editor changes a circuit only through [`Command`]s, which gives
//! undo/redo and a readable log of what changed.

use crate::{
    Assembly, CatalogueRef, Circuit, Component, ComponentKind, ExtraBomLine, Net, Placement,
    PortRef,
};

// Commands are few and short-lived; boxing `Add` would only add noise at every call site.
#[allow(clippy::large_enum_variant)]
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
    /// Rename an external connection such as a manifold port, or relabel an item.
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
    /// Use a catalogue valve type (and part) for a valve, keeping what the drawing requested.
    /// `port_map` lists `(old, new)` for every port that stays connected; the valve's other
    /// ports come off their nets.
    Use {
        id: String,
        valve_type: CatalogueRef,
        part: Option<String>,
        stand_in: bool,
        port_map: Vec<(String, String)>,
    },
    /// Forget that the circuit was imported, e.g. for a copy.
    ClearImport,
    /// Put a component inside the manifold block, or take it out.
    SetInBlock { id: String, inside: bool },
    /// Set an item's part number, description and parts-table position. Blank text clears one.
    SetItemDetails {
        id: String,
        code: Option<String>,
        description: Option<String>,
        position: Option<String>,
    },
    /// Add a bill-of-materials line that isn't a component, after the others.
    AddExtraLine { line: ExtraBomLine },
    /// Replace extra bill-of-materials line `index`, counting from 0.
    SetExtraLine { index: usize, line: ExtraBomLine },
    /// Delete extra bill-of-materials line `index`, counting from 0.
    RemoveExtraLine { index: usize },
    /// Replace an imported circuit's notes. Blank notes are dropped.
    SetNotes { notes: Vec<String> },
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
            Command::Use {
                id,
                valve_type,
                stand_in: true,
                ..
            } => format!("Use {valve_type} as a stand-in for {id}"),
            Command::Use { id, valve_type, .. } => format!("Use {valve_type} for {id}"),
            Command::ClearImport => "Clear the import marks".into(),
            Command::SetInBlock { id, inside: true } => format!("Put {id} in the block"),
            Command::SetInBlock { id, inside: false } => format!("Take {id} out of the block"),
            Command::SetItemDetails { id, .. } => format!("Edit {id}'s parts list entry"),
            Command::AddExtraLine { .. } => "Add an extra line".into(),
            Command::SetExtraLine { index, .. } => format!("Edit extra line {}", index + 1),
            Command::RemoveExtraLine { index } => format!("Delete extra line {}", index + 1),
            Command::SetNotes { .. } => "Edit the import notes".into(),
        }
    }

    /// Port mapping for [`Command::Use`]: same names first, then the rest in order when the
    /// counts left over match.
    pub fn default_port_map(old: &[&str], new: &[&str]) -> Vec<(String, String)> {
        let mut map: Vec<(String, String)> = old
            .iter()
            .filter(|p| new.contains(p))
            .map(|p| (p.to_string(), p.to_string()))
            .collect();
        let old_rest: Vec<_> = old.iter().filter(|p| !new.contains(p)).collect();
        let new_rest: Vec<_> = new.iter().filter(|p| !old.contains(p)).collect();
        if old_rest.len() == new_rest.len() {
            map.extend(
                old_rest
                    .into_iter()
                    .zip(new_rest)
                    .map(|(o, n)| (o.to_string(), n.to_string())),
            );
        }
        map
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
                circuit.in_block.remove(id);
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
                    ComponentKind::Item(item) => {
                        item.label = Some(label.clone()).filter(|l| !l.trim().is_empty());
                    }
                    ComponentKind::Valve(_) => {
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
                    .map(|c| matches!(c.kind, ComponentKind::Valve(_)));
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
            Command::Use {
                id,
                valve_type,
                part,
                stand_in,
                port_map,
            } => {
                for (i, (_, new)) in port_map.iter().enumerate() {
                    if port_map[..i].iter().any(|(_, n)| n == new) {
                        return Err(format!("two ports would both become {new}"));
                    }
                }
                let Some(ComponentKind::Valve(v)) = circuit
                    .components
                    .iter_mut()
                    .find(|c| &c.id == id)
                    .map(|c| &mut c.kind)
                else {
                    return Err(format!("{id} is not a valve in the circuit"));
                };
                let unchanged = v.valve_type.as_ref() == Some(valve_type)
                    && &v.part == part
                    && v.stand_in == *stand_in
                    && port_map.iter().all(|(o, n)| o == n);
                if unchanged {
                    return Err(format!("{id} already uses {valve_type}"));
                }
                v.valve_type = Some(valve_type.clone());
                v.part = part.clone();
                v.stand_in = *stand_in;
                let touched = nets_of(circuit, id);
                for net in &mut circuit.nets {
                    net.ports.retain_mut(|p| {
                        if &p.component != id {
                            return true;
                        }
                        match port_map.iter().find(|(old, _)| *old == p.port) {
                            Some((_, new)) => {
                                p.port = new.clone();
                                true
                            }
                            None => false,
                        }
                    });
                }
                forget(circuit, &touched);
            }
            Command::ClearImport => {
                if circuit.import.take().is_none() {
                    return Err("the circuit was not imported".into());
                }
            }
            Command::SetInBlock { id, inside } => {
                exists(circuit, id)?;
                let changed = if *inside {
                    circuit.in_block.insert(id.clone())
                } else {
                    circuit.in_block.remove(id)
                };
                if !changed {
                    let where_ = if *inside { "in" } else { "out of" };
                    return Err(format!("{id} is already {where_} the block"));
                }
            }
            Command::SetItemDetails {
                id,
                code,
                description,
                position,
            } => {
                let Some(ComponentKind::Item(item)) = circuit
                    .components
                    .iter_mut()
                    .find(|c| &c.id == id)
                    .map(|c| &mut c.kind)
                else {
                    return Err(format!("{id} is not an item in the circuit"));
                };
                let new = [code, description, position].map(|t| text(t.as_deref()));
                let old = [&item.code, &item.description, &item.position];
                if new.iter().zip(old).all(|(n, o)| n == o) {
                    return Err(format!("nothing changed on {id}"));
                }
                [item.code, item.description, item.position] = new;
            }
            Command::AddExtraLine { line } => {
                let n = circuit.bom_extra.len() + 1;
                circuit.bom_extra.push(extra_line(line, n)?);
            }
            Command::SetExtraLine { index, line } => {
                let new = extra_line(line, index + 1)?;
                let old = extra_at(circuit, *index)?;
                if *old == new {
                    return Err(format!("nothing changed on extra line {}", index + 1));
                }
                *old = new;
            }
            Command::RemoveExtraLine { index } => {
                extra_at(circuit, *index)?;
                circuit.bom_extra.remove(*index);
            }
            Command::SetNotes { notes } => {
                let import = circuit
                    .import
                    .as_mut()
                    .ok_or("the circuit was not imported, so it has no import notes")?;
                let notes: Vec<String> = notes.iter().filter_map(|n| text(Some(n))).collect();
                if import.notes == notes {
                    return Err("the import notes are unchanged".into());
                }
                import.notes = notes;
            }
        }
        Ok(())
    }
}

/// Trimmed text, or `None` when blank.
fn text(t: Option<&str>) -> Option<String> {
    t.map(str::trim).filter(|t| !t.is_empty()).map(String::from)
}

/// `line` with its text trimmed, or why it can't be extra line `n`.
fn extra_line(line: &ExtraBomLine, n: usize) -> Result<ExtraBomLine, String> {
    let line = ExtraBomLine {
        position: text(line.position.as_deref()),
        quantity: line.quantity,
        code: text(line.code.as_deref()),
        description: text(line.description.as_deref()),
    };
    if line.quantity < 1 {
        return Err(format!("extra line {n} needs a quantity of at least 1"));
    }
    if line.code.is_none() && line.description.is_none() {
        return Err(format!("extra line {n} needs a code or a description"));
    }
    Ok(line)
}

fn extra_at(circuit: &mut Circuit, index: usize) -> Result<&mut ExtraBomLine, String> {
    circuit
        .bom_extra
        .get_mut(index)
        .ok_or_else(|| format!("there is no extra line {}", index + 1))
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
    generation: u64,
}

impl Document {
    pub fn new(circuit: Circuit) -> Self {
        Self {
            saved: circuit.clone(),
            circuit,
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
        }
    }

    /// Counts changes to the circuit, so anything derived from it can be cached per value.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn apply(&mut self, command: Command) -> Result<(), String> {
        let before = self.circuit.clone();
        command.apply(&mut self.circuit)?;
        self.undo.push((command.describe(), before));
        self.redo.clear();
        self.generation += 1;
        Ok(())
    }

    /// Undo the last edit; returns its description.
    pub fn undo(&mut self) -> Option<String> {
        let (what, before) = self.undo.pop()?;
        let after = std::mem::replace(&mut self.circuit, before);
        self.redo.push((what.clone(), after));
        self.generation += 1;
        Some(what)
    }

    /// Redo the last undone edit; returns its description.
    pub fn redo(&mut self) -> Option<String> {
        let (what, after) = self.redo.pop()?;
        let before = std::mem::replace(&mut self.circuit, after);
        self.undo.push((what.clone(), before));
        self.generation += 1;
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
