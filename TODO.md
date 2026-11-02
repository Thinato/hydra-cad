# To do

Open work after the block outline and mirrored symbols (8e8f237). Tick items off as they land.

## Worth doing next

- [x] **Wire routing.** The AI import traces each net's wires from the drawing into `layout.wires`, and `import_circuit` refuses wires that miss a port, touch another net or leave a net in pieces (#6). Since then:
  - [x] Drawing wires by hand in the editor: draw, delete, reset a net to automatic, join nets by ending on them (#14).
  - [x] Smarter automatic routing for untraced nets: a grid router keeps clear of symbols, labels and other nets and joins ports as a tree (#19).
  - [x] Traced wires are checked against untraced nets' automatic lines (#17).
  - [x] The through-part warning only lets off a segment leaving its own port outward (#17).
  - [x] Block ports get no junction dot (#18).
  - [x] Wires of different nets hop where they cross (#18).
  - [x] Pilot and drain wires are drawn dashed; the kind is set per traced wire (#15).
  - [x] A traced wire along the block outline or across a designator is warned about (#17).
  - [ ] Moving, rotating or swapping a part still drops its nets' drawn wires (the editor says so, and Undo brings them back). Keeping the wire and moving only its end would keep the engineer's work.
  - [ ] Automatic wires stay solid. A pilot or drain line shows dashed only once it is traced.
- [x] **Connect the drawn block to the 3D block.** Only parts in `in_block` are expected on the 3D block (#3), and mounting a valve on a face ticks it In block in the same undo step; unticking never unmounts, it warns (#13).
- [x] **New parts inside the outline** join the block in the same undo step (#8).

## Design system

The look is in [docs/design.md](docs/design.md) and `crates/hydra/src/theme.rs`, which installs the colours, the bundled IBM Plex fonts and the type scale.

- [ ] Restyle the tabs on the theme, one PR each: Account and 3D still to do.
  - [x] Proposals, with the data that exists today. It added the painted status marks (▲ ■ ⇄ ✓ ◆ ● ✕, `theme::Mark`), status chips, banners, and the primary and danger buttons to `theme.rs`.
  - [x] Schematic, around the sheet: toolbar groups, the import banner, the parts list, the inspector, the issues panel and the status bar. It added `theme::group`, `section` and `list_row`, and a button in `banner`. The sheet itself is the next item.
  - [x] Catalogue: the toolbar with search, the valve-type table with a status per row, the detail panel, the new-version editor as a fields table, and the drop overlay on palette colours. It moved the Proposals tables to `theme::table`, `table_row`, `cell` and `column_title`, and the symbol preview to `schematic::symbol_preview`. The app-wide status line above the tabs (where import and drop errors show) is still plain text.
- [x] Schematic sheet as on the boards: an ISO A sheet (A4 to A0) on the desk with the 5 mm grid, working lines heavier than pilot and drain, a short dash of its own for drain lines, and a 2 px selection frame. The SVG and PDF are the same sheet, lettered in Plex.
- [ ] The installers ship no licence files for the fonts compiled into the binary: Plex's OFL is readable in the app, but egui's own fonts (Ubuntu, Hack, Noto Emoji, emoji-icon-font) aren't. Add the licences to the MSI and DMG, or show them beside Plex's.
- [x] Schematic: a title block in the sheet's corner (name, schema, revision, drawn by, checked), also in the SVG and PDF exports. Still open:
  - [ ] The date and who drew and checked a revision: the editor opens a circuit without its revision's author and time (`OpenedCircuit`), so the block shows only what it knows (the AI assistant for an import, "not yet" checked at its first revision) and "—" for the rest.
  - [ ] Designators on the sheet in Plex Mono SemiBold, as the boards letter them.
- [ ] Record checks: a revision flag (and the revision author/time, already on TODO) so the title block can say who checked which revision; stored-shape change (rule 2). Also restoring revision 1 of an import should count as unchecked again — today `unchecked()` (import && revision == 1) and the banner miss it.
- [ ] Proposals, "What the assistant did": hydra-mcp records each step of a proposal (datasheet read, symbol matched, fields filled, parts listed, checks run) with the datasheet page it came from, and the detail view lists them with their result. Each field then shows its source page, and parameters show as a table instead of raw JSON. Stored with the proposal, it changes a stored shape: schema bump and gate migration (rule 2).
- [ ] Proposals, assistant activity feed: hydra-mcp logs each tool call that matters to an engineer (datasheet read, proposal submitted, circuit imported, rejection read and resubmitted) in a new table; a panel on the Proposals tab lists today's, linking to the proposal or circuit, with when the assistant last called.

## Smaller

- [x] Block outlines are cut round outside parts: L notches (#8), stepped corners and U cuts from an edge (#16). The editor warns about each part the outline is cut round.
- [x] Block port labels sit outside the block, the dash-dot line breaks round port circles (#8), and labels dodge a wire arriving from outside (#18).
- [x] Designators keep clear of wires and status frames (#4); item labels, status words and port names now do too, and slide along their part when they must (#18). Still open:
  - [ ] Where no clear spot is left they stay under the part, e.g. FC1–FC3's status words and HM1–HM3's labels in demo-import-motors, whose traced wires box them in.
- [x] The editor edits the extra bill-of-materials lines, an item's code, description and position, and the import notes (#7).
- [x] `submit_proposal` refuses a second pending proposal for the same valve type and base version (#5).
- [x] Pilot ports symbols lacked: relief valves and compensated flow controls can have an external pilot port X; block ports can be plugged (#15, circuit schema 5).
  - [ ] The two live imported circuits still draw their 4.1 and 9.1 pilot lines without a port, and MH450001's T rail without its plug: they need catalogue versions with port X and a new revision, checked by the engineer.
  - [ ] A vented or remote-setting relief (internal pilot kept, X as a vent) isn't modelled; the assistant notes it.
- [x] The hydra-mcp test `a_stuck_renderer_is_killed` finds its own child by a per-process marker (#12).
- [x] The editor renders the schematic once per edit, not several times a frame (#21). Still open:
  - [ ] The wire checks lay the circuit out three times per edit, and the router keeps a one-entry memo for them; one shared render would let both go.

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
