# To do

Open work after the block outline and mirrored symbols (8e8f237). Tick items off as they land.

## Worth doing next

- [ ] **Wire routing.** Wires are drawn automatically: a trunk from a net's first port, then an elbow into each other port. On imported circuits this gives long elbows, lines through symbols and crossings the drawing doesn't have. Either:
  - route automatically around symbols and along rows; or
  - let the engineer, and the import, draw wires. `layout.wires` is already read and drawn, but nothing writes it yet.
- [ ] **Connect the drawn block to the 3D block.** With a 3D block chosen (`assembly`), a valve left outside `in_block` still gets "not mounted on the block". A valve mounted on a block face but not in `in_block` raises no warning. Make the two agree, or warn when they don't.
- [ ] **New parts inside the outline.** Toolbar adds land in the middle of the view, usually inside the block outline, and are flagged at once ("drawn inside its outline"). Tick "In block" automatically when a new part lands inside the outline.

## Smaller

- [ ] The block outline is always a rectangle. Real block boundaries can be L-shaped, so a correct drawing can flag outside parts as drawn inside.
- [ ] A block port on the outline's bottom edge shows its label inside the block, and the dash-dot line runs through its circle.
- [ ] Designators overlap wires on some parts, e.g. RV1 in the demo manifold.
- [ ] The editor can't edit the extra bill-of-materials lines (block, plugs, coils), an item's code, description or position, or the import notes.
- [ ] `submit_proposal` accepts a second pending proposal for the same valve type and base version. Only one can be approved. Decide whether to warn or refuse.

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
