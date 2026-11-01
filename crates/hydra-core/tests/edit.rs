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
        kind: ComponentKind::Port {
            label: id.into(),
            plugged: false,
        },
    }
}

fn add(id: &str, x: f32) -> Command {
    Command::Add {
        component: port(id),
        placement: Placement {
            x,
            y: 0.0,
            ..Placement::default()
        },
        in_block: false,
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
        in_block: Default::default(),
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

#[test]
fn parts_go_in_and_out_of_the_block() {
    let mut d = doc(&["P", "T"]);
    let put = |id: &str, inside| Command::SetInBlock {
        id: id.into(),
        inside,
    };
    d.apply(put("P", true)).unwrap();
    assert_eq!(
        d.apply(put("P", true)).unwrap_err(),
        "P is already in the block"
    );
    assert_eq!(
        d.apply(put("T", false)).unwrap_err(),
        "T is already out of the block"
    );
    assert_eq!(
        d.apply(put("Q", true)).unwrap_err(),
        "Q is not in the circuit"
    );
    d.apply(put("T", true)).unwrap();
    d.apply(put("T", false)).unwrap();
    assert_eq!(d.undo().as_deref(), Some("Take T out of the block"));
    assert!(d.circuit.in_block.contains("T"));
    // Deleting a part takes it out of the block too.
    d.apply(Command::Remove { id: "P".into() }).unwrap();
    assert_eq!(Vec::from_iter(&d.circuit.in_block), ["T"]);
    // A part can be added straight into the block; one undo takes back both.
    let Command::Add {
        component,
        placement,
        ..
    } = add("Q", 5.0)
    else {
        unreachable!()
    };
    d.apply(Command::Add {
        component,
        placement,
        in_block: true,
    })
    .unwrap();
    assert_eq!(Vec::from_iter(&d.circuit.in_block), ["Q", "T"]);
    d.undo();
    assert!(d.circuit.component("Q").is_none());
    assert_eq!(Vec::from_iter(&d.circuit.in_block), ["T"]);
    assert!(
        d.circuit
            .validate(&Catalogue::default())
            .iter()
            .all(|i| !matches!(i, Issue::InBlockUnknown(_)))
    );

    // Older documents have nothing in the block; an unknown id is an error.
    let mut c = Circuit::from_json(r#"{"schema": 3, "name": "t", "components": []}"#).unwrap();
    assert!(c.in_block.is_empty());
    c.in_block.insert("Z".into());
    let issues = c.validate(&Catalogue::default());
    assert_eq!(issues, [Issue::InBlockUnknown("Z".into())]);
    assert_eq!(issues[0].severity(), Severity::Error);
    assert!(
        Circuit::from_json(&c.to_json())
            .unwrap()
            .in_block
            .contains("Z")
    );
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
        mirror: false,
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
        ComponentKind::Port {
            label: "P".into(),
            plugged: false
        }
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
        kind: NetKind::Working,
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
        in_block: false,
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
    assert_eq!(
        d.undo_label(),
        Some("Mount V1 on F1 and put it in the block")
    );
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

#[test]
fn mounting_a_valve_puts_it_in_the_block_in_one_step() {
    let mut d = doc(&[]);
    d.apply(valve("V1")).unwrap();
    d.apply(valve("V2")).unwrap();
    d.apply(set_block("blk", 1)).unwrap();
    let before = d.circuit.clone();
    d.apply(mount("V1", Some("F1"))).unwrap();
    assert!(d.circuit.in_block.contains("V1"));
    // One undo takes off the mount and the tick together.
    assert_eq!(
        d.undo().as_deref(),
        Some("Mount V1 on F1 and put it in the block")
    );
    assert_eq!(d.circuit, before);
    // Redo puts both back.
    assert_eq!(
        d.redo().as_deref(),
        Some("Mount V1 on F1 and put it in the block")
    );
    assert!(d.circuit.in_block.contains("V1"));
    assert_eq!(mounts(&d), [("V1".into(), "F1".into())]);
    // A valve bumped off its face keeps its tick.
    d.apply(mount("V2", Some("F1"))).unwrap();
    assert_eq!(mounts(&d), [("V2".into(), "F1".into())]);
    assert!(d.circuit.in_block.contains("V1"));
    d.undo();
    d.undo();
    assert_eq!(d.circuit, before);

    // A valve already in the block stays in it when the mount is undone.
    d.apply(Command::SetInBlock {
        id: "V2".into(),
        inside: true,
    })
    .unwrap();
    let before = d.circuit.clone();
    d.apply(mount("V2", Some("F2"))).unwrap();
    assert_eq!(d.undo_label(), Some("Mount V2 on F2"));
    d.undo();
    assert_eq!(d.circuit, before);
    assert!(d.circuit.in_block.contains("V2"));

    // Unmounting leaves the valve in the block.
    d.apply(mount("V1", Some("F1"))).unwrap();
    d.apply(mount("V1", None)).unwrap();
    assert!(d.circuit.in_block.contains("V1"));

    // Unticking In block keeps the mount, and validation says so.
    d.apply(mount("V1", Some("F1"))).unwrap();
    d.apply(Command::SetInBlock {
        id: "V1".into(),
        inside: false,
    })
    .unwrap();
    assert_eq!(mounts(&d), [("V1".into(), "F1".into())]);
    assert!(
        d.circuit
            .validate(&Catalogue::default())
            .contains(&Issue::MountedOutsideBlock("V1".into()))
    );
}

fn item(id: &str) -> Command {
    Command::Add {
        component: Component {
            id: id.into(),
            kind: ComponentKind::Item(Item {
                symbol: SymbolDef::TestPoint,
                label: None,
                code: None,
                description: None,
                position: None,
            }),
        },
        placement: Placement::default(),
        in_block: false,
    }
}

fn details(id: &str, code: &str, description: &str) -> Command {
    Command::SetItemDetails {
        id: id.into(),
        code: code.into(),
        description: description.into(),
        position: " 14 ".into(),
    }
}

#[test]
fn item_details_are_set_trimmed_cleared_and_undone() {
    let mut d = doc(&["P"]);
    d.apply(item("M1")).unwrap();
    d.apply(details("M1", " DEMO-MP-04 ", "Test point"))
        .unwrap();
    let get = |d: &Document| match &d.circuit.component("M1").unwrap().kind {
        ComponentKind::Item(i) => (i.code.clone(), i.description.clone(), i.position.clone()),
        _ => unreachable!(),
    };
    let set = (
        Some("DEMO-MP-04".to_string()),
        Some("Test point".to_string()),
        Some("14".to_string()),
    );
    assert_eq!(get(&d), set);
    assert_eq!(d.undo_label(), Some("Edit M1's parts list entry"));
    // The item now shows in the bill of materials.
    let bom = d.circuit.bom(&Catalogue::default());
    assert_eq!(bom[0].part_number.as_deref(), Some("DEMO-MP-04"));
    assert_eq!(bom[0].position.as_deref(), Some("14"));

    // Blank text clears a field.
    d.apply(details("M1", "DEMO-MP-04", "  ")).unwrap();
    assert_eq!(get(&d).1, None);
    d.undo();
    assert_eq!(get(&d), set);

    // Refused: no change, not an item, not there.
    let before = d.circuit.clone();
    let e = d
        .apply(details("M1", "DEMO-MP-04 ", "Test point"))
        .unwrap_err();
    assert_eq!(e, "nothing changed on M1");
    let e = d.apply(details("P", "X", "")).unwrap_err();
    assert_eq!(e, "P is not an item in the circuit");
    assert!(d.apply(details("Z", "X", "")).is_err());
    assert_eq!(d.circuit, before);
}

fn line(position: &str, quantity: u32, code: &str, description: &str) -> ExtraBomLine {
    let opt = |t: &str| Some(t.to_string());
    ExtraBomLine {
        position: opt(position),
        quantity,
        code: opt(code),
        description: opt(description),
    }
}

#[test]
fn extra_lines_are_added_edited_removed_and_undone() {
    let mut d = doc(&[]);
    d.apply(Command::AddExtraLine {
        line: line(" 1 ", 1, "DEMO-BLOCK-450", "Manifold block"),
    })
    .unwrap();
    d.apply(Command::AddExtraLine {
        line: line("20", 4, "DEMO-PLUG-G1/4", " "),
    })
    .unwrap();
    assert_eq!(
        d.circuit.bom_extra,
        [line("1", 1, "DEMO-BLOCK-450", "Manifold block"), {
            let mut l = line("20", 4, "DEMO-PLUG-G1/4", "");
            l.description = None;
            l
        }]
    );
    assert_eq!(d.undo_label(), Some("Add an extra line"));

    d.apply(Command::SetExtraLine {
        index: 1,
        line: line("20", 6, "DEMO-PLUG-G1/4", "Plug G1/4"),
    })
    .unwrap();
    assert_eq!(d.circuit.bom_extra[1].quantity, 6);
    assert_eq!(d.undo_label(), Some("Edit extra line 2"));
    // The bill of materials shows the edit.
    let bom = d.circuit.bom(&Catalogue::default());
    assert_eq!(bom[1].quantity, 6);
    assert_eq!(bom[1].description, "Plug G1/4");

    d.apply(Command::RemoveExtraLine { index: 0 }).unwrap();
    assert_eq!(d.circuit.bom_extra.len(), 1);
    assert_eq!(d.undo_label(), Some("Delete extra line 1"));
    d.undo();
    assert_eq!(d.circuit.bom_extra[0].position.as_deref(), Some("1"));
    d.undo();
    assert_eq!(d.circuit.bom_extra[1].quantity, 4);

    // Refused, leaving the lines as they are.
    let before = d.circuit.clone();
    let refusals = [
        (
            Command::AddExtraLine {
                line: line("3", 0, "DEMO-SEAL", ""),
            },
            "extra line 3 needs a quantity of at least 1",
        ),
        (
            Command::SetExtraLine {
                index: 0,
                line: ExtraBomLine {
                    position: None,
                    quantity: 1,
                    code: Some(" ".into()),
                    description: None,
                },
            },
            "extra line 1 needs a code or a description",
        ),
        (
            Command::SetExtraLine {
                index: 0,
                line: line("1 ", 1, "DEMO-BLOCK-450", "Manifold block"),
            },
            "nothing changed on extra line 1",
        ),
        (
            Command::SetExtraLine {
                index: 5,
                line: line("1", 1, "X", "Y"),
            },
            "there is no extra line 6",
        ),
        (
            Command::RemoveExtraLine { index: 2 },
            "there is no extra line 3",
        ),
    ];
    for (command, why) in refusals {
        assert_eq!(d.apply(command).unwrap_err(), why);
    }
    assert_eq!(d.circuit, before);
}

#[test]
fn import_notes_are_edited_only_on_imported_circuits() {
    let mut d = doc(&[]);
    let set = |notes: &[&str]| Command::SetNotes {
        notes: notes.iter().map(|n| n.to_string()).collect(),
    };
    assert_eq!(
        d.apply(set(&["a"])).unwrap_err(),
        "the circuit was not imported, so it has no import notes"
    );
    assert_eq!(d.circuit.import, None, "no import marks appear");

    d.circuit.import = Some(Import {
        notes: vec!["Row 12 is illegible (demo).".into()],
    });
    d.apply(set(&[
        "Row 12 is illegible (demo).",
        "  ",
        " Checked row 7. ",
    ]))
    .unwrap();
    let notes = |d: &Document| d.circuit.import.as_ref().unwrap().notes.clone();
    assert_eq!(notes(&d), ["Row 12 is illegible (demo).", "Checked row 7."]);
    assert_eq!(d.undo_label(), Some("Edit the import notes"));
    assert_eq!(
        d.apply(set(&["Row 12 is illegible (demo).", "Checked row 7. "]))
            .unwrap_err(),
        "the import notes are unchanged"
    );
    // Clearing them keeps the circuit marked as imported.
    d.apply(set(&[])).unwrap();
    assert_eq!(d.circuit.import, Some(Import::default()));
    d.undo();
    d.undo();
    assert_eq!(notes(&d), ["Row 12 is illegible (demo)."]);
}

#[test]
fn nets_become_pilot_or_drain_lines_and_merging_keeps_the_kind() {
    let mut d = doc(&["A", "B", "C", "D"]);
    d.apply(connect("A", "B")).unwrap();
    d.apply(connect("C", "D")).unwrap();
    let kind = |net: &str, kind| Command::SetNetKind {
        net: net.into(),
        kind,
    };
    assert_eq!(
        d.apply(kind("n9", NetKind::Pilot)),
        Err("net n9 is not in the circuit".into())
    );
    // New nets are working lines.
    assert_eq!(
        d.apply(kind("n1", NetKind::Working)),
        Err("net n1 is already a working line".into())
    );
    d.apply(kind("n2", NetKind::Drain)).unwrap();
    assert_eq!(d.undo_label(), Some("Make net n2 a drain line"));
    assert_eq!(d.circuit.nets[1].kind, NetKind::Drain);
    // Joining a working line to a drain line keeps the drain, whichever comes first.
    d.apply(connect("A", "C")).unwrap();
    assert_eq!(d.circuit.nets.len(), 1);
    assert_eq!(d.circuit.nets[0].kind, NetKind::Drain);
    d.undo();
    d.undo();
    assert_eq!(d.circuit.nets[1].kind, NetKind::Working);
    // A pilot line merged into a working one stays a pilot line.
    d.apply(kind("n1", NetKind::Pilot)).unwrap();
    d.apply(connect("C", "A")).unwrap();
    assert_eq!(d.circuit.nets[0].kind, NetKind::Pilot);
}

#[test]
fn only_external_connections_are_plugged() {
    let mut d = doc(&["P"]);
    d.apply(item("X1")).unwrap();
    let plug = |id: &str, plugged| Command::SetPlugged {
        id: id.into(),
        plugged,
    };
    assert_eq!(
        d.apply(plug("X1", true)),
        Err("X1 is not an external connection in the circuit".into())
    );
    assert_eq!(d.apply(plug("P", false)), Err("P is already open".into()));
    d.apply(plug("P", true)).unwrap();
    assert_eq!(d.undo_label(), Some("Plug P"));
    assert_eq!(
        d.circuit.component("P").unwrap().kind,
        ComponentKind::Port {
            label: "P".into(),
            plugged: true
        }
    );
    assert_eq!(d.apply(plug("P", true)), Err("P is already plugged".into()));
    d.undo();
    assert_eq!(
        d.circuit.component("P").unwrap().kind,
        ComponentKind::Port {
            label: "P".into(),
            plugged: false
        }
    );
}
