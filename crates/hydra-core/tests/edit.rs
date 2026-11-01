use hydra_core::*;

fn pr(component: &str, port: &str) -> PortRef {
    PortRef {
        component: component.into(),
        port: port.into(),
    }
}

fn port(id: &str) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Port { label: id.into() },
    }
}

fn add(id: &str, x: f32) -> Command {
    Command::Add {
        component: port(id),
        placement: Placement {
            x,
            y: 0.0,
            rotation: Rotation::R0,
        },
    }
}

fn connect(a: &str, b: &str) -> Command {
    Command::Connect {
        a: pr(a, PORT_PIN),
        b: pr(b, PORT_PIN),
    }
}

fn doc(ids: &[&str]) -> Document {
    let mut d = Document::new(Circuit {
        schema: SCHEMA_VERSION,
        assembly: None,
        name: "t".into(),
        components: vec![],
        nets: vec![],
        layout: Layout::default(),
        bom_extra: vec![],
        import: None,
    });
    for (i, id) in ids.iter().enumerate() {
        d.apply(add(id, i as f32 * 10.0)).unwrap();
    }
    d
}

fn nets(d: &Document) -> Vec<(String, Vec<String>)> {
    d.circuit
        .nets
        .iter()
        .map(|n| {
            (
                n.id.clone(),
                n.ports.iter().map(|p| p.component.clone()).collect(),
            )
        })
        .collect()
}

#[test]
fn connecting_creates_extends_and_merges_nets() {
    let mut d = doc(&["A", "B", "C", "D"]);
    d.apply(connect("A", "B")).unwrap();
    d.apply(connect("C", "D")).unwrap();
    assert_eq!(
        nets(&d),
        [
            ("n1".into(), vec!["A".into(), "B".into()]),
            ("n2".into(), vec!["C".into(), "D".into()])
        ]
    );
    // Joining a port of each net merges them into the first.
    d.apply(connect("D", "A")).unwrap();
    assert_eq!(
        nets(&d),
        [(
            "n2".into(),
            vec!["C".into(), "D".into(), "A".into(), "B".into()]
        )]
    );
    assert_eq!(
        d.apply(connect("B", "C")).unwrap_err(),
        "B.port and C.port are already connected"
    );
    assert!(d.apply(connect("A", "A")).is_err());
    assert!(d.apply(connect("A", "Z")).is_err());
}

#[test]
fn removing_a_component_drops_its_connections_and_dangling_nets() {
    let mut d = doc(&["A", "B", "C"]);
    d.apply(connect("A", "B")).unwrap();
    d.apply(connect("B", "C")).unwrap();
    d.apply(Command::Remove { id: "B".into() }).unwrap();
    assert_eq!(nets(&d), [("n1".into(), vec!["A".into(), "C".into()])]);
    d.apply(Command::Remove { id: "C".into() }).unwrap();
    assert!(d.circuit.nets.is_empty(), "a one-port net is no net");
    assert!(!d.circuit.layout.components.contains_key("C"));
}

#[test]
fn disconnecting_removes_one_port() {
    let mut d = doc(&["A", "B", "C"]);
    d.apply(connect("A", "B")).unwrap();
    d.apply(connect("A", "C")).unwrap();
    d.apply(Command::Disconnect {
        port: pr("C", PORT_PIN),
    })
    .unwrap();
    assert_eq!(nets(&d), [("n1".into(), vec!["A".into(), "B".into()])]);
    d.apply(Command::Disconnect {
        port: pr("A", PORT_PIN),
    })
    .unwrap();
    assert!(d.circuit.nets.is_empty());
    assert_eq!(
        d.apply(Command::Disconnect {
            port: pr("A", PORT_PIN)
        })
        .unwrap_err(),
        "A.port is not connected"
    );
}

#[test]
fn moving_rotating_and_relabelling() {
    let mut d = doc(&["A", "B"]);
    d.apply(connect("A", "B")).unwrap();
    d.circuit
        .layout
        .wires
        .insert("n1".into(), vec![vec![[0.0, 0.0], [10.0, 0.0]]]);
    let to = Placement {
        x: 5.0,
        y: 7.5,
        rotation: Rotation::R90,
    };
    d.apply(Command::Place {
        id: "A".into(),
        placement: to,
    })
    .unwrap();
    assert_eq!(d.circuit.layout.components["A"], to);
    assert!(
        d.circuit.layout.wires.is_empty(),
        "hand-drawn wires of a moved part are dropped"
    );
    d.apply(Command::SetLabel {
        id: "A".into(),
        label: "P".into(),
    })
    .unwrap();
    assert_eq!(
        d.circuit.component("A").unwrap().kind,
        ComponentKind::Port { label: "P".into() }
    );
    assert!(
        d.apply(Command::Place {
            id: "Z".into(),
            placement: to
        })
        .is_err()
    );
    d.apply(Command::Rename {
        name: " Germani block ".into(),
    })
    .unwrap();
    assert_eq!(d.circuit.name, "Germani block");
    assert!(d.apply(Command::Rename { name: " ".into() }).is_err());
}

#[test]
fn refused_commands_change_nothing_and_are_not_undoable() {
    let mut d = doc(&["A"]);
    let before = d.circuit.clone();
    assert!(d.apply(add("A", 0.0)).is_err());
    assert_eq!(d.circuit, before);
    assert_eq!(d.undo_label(), Some("Add A"));
}

#[test]
fn undo_redo_and_dirty_tracking() {
    let mut d = doc(&["A", "B"]);
    d.mark_saved_as(d.circuit.clone());
    assert!(!d.is_dirty());
    d.apply(connect("A", "B")).unwrap();
    assert!(d.is_dirty());
    assert_eq!(d.undo(), Some("Connect A.port to B.port".into()));
    assert!(!d.is_dirty(), "undoing back to the saved state is clean");
    assert_eq!(d.redo_label(), Some("Connect A.port to B.port"));
    d.redo();
    assert_eq!(d.circuit.nets.len(), 1);
    // Editing while a save is in flight leaves the later edit dirty.
    let sent = d.circuit.clone();
    d.apply(Command::Remove { id: "A".into() }).unwrap();
    d.mark_saved_as(sent);
    assert!(d.is_dirty());
    d.undo();
    assert!(!d.is_dirty());
    // A new edit clears the redo stack.
    d.undo();
    d.apply(Command::Remove { id: "B".into() }).unwrap();
    assert_eq!(d.redo(), None);
    // Undo walks all the way back.
    while d.undo().is_some() {}
    assert!(d.circuit.components.is_empty());
}

#[test]
fn next_id_fills_gaps() {
    assert_eq!(next_id(["V1", "V3"].into_iter(), "V"), "V2");
    assert_eq!(next_id(std::iter::empty(), "K"), "K1");
}

#[test]
fn edits_leave_untouched_nets_alone_and_merging_keeps_names() {
    let mut d = doc(&["A", "B", "C", "D", "E"]);
    // A one-port net from a loaded file is a validation matter, not something an edit removes.
    d.circuit.nets.push(Net {
        id: "lone".into(),
        name: None,
        ports: vec![pr("E", PORT_PIN)],
    });
    d.apply(connect("A", "B")).unwrap();
    d.apply(connect("C", "D")).unwrap();
    d.circuit.nets[2].name = Some("supply".into());
    d.apply(Command::Place {
        id: "A".into(),
        placement: Placement::default(),
    })
    .unwrap();
    d.apply(connect("A", "C")).unwrap();
    assert!(d.circuit.nets.iter().any(|n| n.id == "lone"));
    let merged = d.circuit.nets.iter().find(|n| n.ports.len() == 4).unwrap();
    assert_eq!(merged.name.as_deref(), Some("supply"));
}

fn valve(id: &str) -> Command {
    Command::Add {
        component: Component {
            id: id.into(),
            kind: ComponentKind::valve(CatalogueRef::new("vt", 1)),
        },
        placement: Placement::default(),
    }
}

fn set_block(key: &str, version: u32) -> Command {
    Command::SetBlock {
        block: Some(CatalogueRef::new(key, version)),
    }
}

fn mount(component: &str, face: Option<&str>) -> Command {
    Command::Mount {
        component: component.into(),
        face: face.map(Into::into),
    }
}

fn mounts(d: &Document) -> Vec<(String, String)> {
    let a = d.circuit.assembly.as_ref().expect("a block");
    a.mounts.clone().into_iter().collect()
}

#[test]
fn mounting_valves_on_a_block() {
    let mut d = doc(&["P"]);
    d.apply(valve("V1")).unwrap();
    d.apply(valve("V2")).unwrap();
    assert_eq!(
        d.apply(mount("V1", Some("F1"))),
        Err("choose a block for the circuit first".into())
    );
    d.apply(set_block("blk", 1)).unwrap();
    assert_eq!(
        d.apply(set_block("blk", 1)),
        Err("the circuit already uses that block".into())
    );
    d.apply(mount("V1", Some("F1"))).unwrap();
    assert_eq!(d.undo_label(), Some("Mount V1 on F1"));
    // A valve put on a taken face replaces the one there, in one undoable step.
    d.apply(mount("V2", Some("F1"))).unwrap();
    assert_eq!(mounts(&d), [("V2".into(), "F1".into())]);
    d.undo();
    assert_eq!(mounts(&d), [("V1".into(), "F1".into())]);
    assert_eq!(
        d.apply(mount("V1", Some("F1"))),
        Err("V1 is already on F1".into())
    );
    assert_eq!(
        d.apply(mount("P", Some("F2"))),
        Err("P is not a valve".into())
    );
    assert_eq!(
        d.apply(mount("V9", Some("F2"))),
        Err("V9 is not in the circuit".into())
    );
    assert_eq!(d.apply(mount("V2", None)), Err("V2 is not mounted".into()));
    // Moving a valve to another face frees the first one.
    d.apply(mount("V1", Some("F2"))).unwrap();
    d.apply(mount("V2", Some("F1"))).unwrap();
    assert_eq!(
        mounts(&d),
        [("V1".into(), "F2".into()), ("V2".into(), "F1".into())]
    );

    // A new version of the same block keeps the mounts; another block starts empty.
    d.apply(set_block("blk", 2)).unwrap();
    assert_eq!(mounts(&d).len(), 2);
    d.apply(set_block("other", 1)).unwrap();
    assert_eq!(mounts(&d), []);
    d.undo();
    assert_eq!(mounts(&d).len(), 2);

    // Deleting a valve takes it off its face.
    d.apply(Command::Remove { id: "V1".into() }).unwrap();
    assert_eq!(mounts(&d), [("V2".into(), "F1".into())]);
    d.apply(mount("V2", None)).unwrap();
    assert_eq!(d.undo_label(), Some("Unmount V2"));
    d.apply(Command::SetBlock { block: None }).unwrap();
    assert_eq!(d.circuit.assembly, None);
    assert_eq!(d.undo_label(), Some("Remove the block"));
}
