# To do

Open work after the block outline and mirrored symbols (8e8f237). Tick items off as they land.

## Worth doing next

- [x] **Wire routing.** The AI import traces each net's wires from the drawing into `layout.wires`, and `import_circuit` refuses wires that miss a port, touch another net or leave a net in pieces (#6). Still open:
  - [ ] Drawing wires by hand in the editor. The editor only drops a net's drawn wires when its ports move.
  - [ ] Smarter automatic routing for nets left untraced (long elbows, lines through symbols).
  - [ ] Traced wires aren't checked against the automatic elbows of untraced nets, so a traced wire can lie along an untraced net's elbow.
  - [ ] The through-part warning skips a whole segment that ends on one of the part's own ports, even if it crosses the symbol to get there.
  - [ ] A block port gets a junction dot inside its circle when a traced wire passes through it, because the port counts as an arm. Drawings show an open circle there.
  - [ ] Crossings are drawn plain. Drawings mark one line hopping over another with a small arc, which would make it clear the lines aren't joined.
  - [ ] Pilot and drain lines are drawn solid like any other wire. Drawings dash them.
  - [ ] A traced wire can run along the block outline (P in demo-import-motors runs up the outline's left edge for 4 mm), and a part's own traced wire can cross its label (T in demo-import-motors).
- [x] **Connect the drawn block to the 3D block.** Only parts in `in_block` are expected on the 3D block; a valve mounted on it but drawn outside warns, and a block with nothing in `in_block` gets one warning (#3). Still open:
  - [ ] Decide whether mounting a valve in the 3D tab should tick "In block" (or the reverse). Today the engineer settles each mismatch by hand.
- [x] **New parts inside the outline** join the block in the same undo step (#8).

## Smaller

- [x] L-shaped blocks: the outline is cut round an outside part standing in a corner, and the import warns about each such part (#8). Still open:
  - [ ] U-shaped and stepped outlines: one rectangular notch per corner, so parts needing more are still flagged.
- [x] Block port labels sit outside the block on every edge, and the dash-dot line breaks round port circles (#8). Still open:
  - [ ] A port label moved outside doesn't avoid a wire arriving at the port from outside.
- [x] Designators keep clear of wires and status frames (#4). Still open:
  - [ ] Status words, item labels and port names can still overlap wires (the status line under 1.1, X1's "X 0.5 mm"). An orifice's value label sits on its own wire when the orifice is vertical. A gauge's label sits under its port, so a wire leaving the port downwards runs through the label.
- [x] The editor edits the extra bill-of-materials lines, an item's code, description and position, and the import notes (#7).
- [x] `submit_proposal` refuses a second pending proposal for the same valve type and base version (#5).
- [ ] Some valves have pilot ports their symbol lacks. A relief valve piloted from another line, or a compensator with a second pilot, can't be connected the way the drawing shows. The datasheet proposal should give those symbols the extra port. Plugged block ports (the end of a rail at the block's edge) aren't modelled either.
- [ ] The hydra-mcp test `a_stuck_renderer_is_killed` looks for its child with `pgrep -f "sleep 5.0371"`, so two checkouts running the suite at once can fail it. Give each run a unique marker.
- [ ] Every editor frame draws each part several times (`render_circuit`, `render_parts`, `outline_issues`, `block_outline`). Fine at tens of parts; share one `parts_and_wires` result per frame if it ever shows.

## Bigger items deferred earlier

- [ ] Packaging and signing:
  - the Windows MSI has never been built (the first `release.yml` run is its test), and the exe has no embedded icon;
  - Windows signing needs Azure Trusted Signing or DigiCert KeyLocker;
  - macOS signing and notarization have never run with real certificates;
  - where `latest.json` is hosted is undecided.
- [ ] Backups:
  - the `ops/backup` scripts have only been parsed by CI and never run;
  - the target machine, retention and service account are undecided;
  - there is no WAL archiving.
- [ ] Real block 3D models: the Germani and BMH STEP files, with their mount-face frames, are still missing.
