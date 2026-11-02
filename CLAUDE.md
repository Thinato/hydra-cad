# CLAUDE.md

Guidance for Claude Code sessions and subagents working in this repository. README.md is the user-facing manual (features, running, installing). This file covers what you need to change the code safely. Where README already says something, this file points to it rather than repeating it.

## What Hydra is

Hydra is a desktop CAD app for hydraulic circuits, used by engineers who design manifold blocks:
- It draws circuits as ISO 1219 schematics from a shared, versioned valve catalogue.
- It shows blocks with their valves mounted in 3D.
- It keeps every circuit's full revision history in one shared Postgres database.

It is written in Rust with egui, for Windows 10/11 and macOS 15+. SolidWorks stays the modelling tool: Hydra only renders exported models. It does not simulate.

An AI assistant, connected through the `hydra-mcp` server, does two jobs:
- It fills in catalogue entries from manufacturer datasheets. An engineer approves each one.
- It turns photos or PDFs of circuit drawings into Hydra circuits.

Hydra itself has no PDF or drawing parser.

**Main objective:** an engineer must be able to trust what Hydra shows.
- A schematic matches its source drawing symbol for symbol.
- Every valve says whether the catalogue really has it: normal, amber (part not listed), red (missing), or a stand-in.
- Nothing in the catalogue or in history is silently changed or lost.

When in doubt, prefer refusing or warning in plain words over guessing.

## Workspace map

| Crate | Role | Key files |
| --- | --- | --- |
| `crates/hydra-core` | Domain model. No UI, no database. | `circuit.rs` (the circuit document and `SCHEMA_VERSION`, plus validation `Issue`s), `symbol.rs` (`SymbolDef` kinds and ports), `catalogue.rs` (versioned rows, `key@version` refs, ratings and functions), `edit.rs` (`Command`s and `Document` with undo), `status.rs` (valve status, stand-ins), `import.rs` (checks on AI-imported circuits), `proposal.rs` (catalogue proposals), `bom.rs` |
| `crates/hydra-symbols` | Symbol definitions → lines, circles and text in mm (x right, y down); circuit layout, block outline, the sheet and its title block, SVG/PDF | `lib.rs` (`render_symbol`, `render_component`, `render_circuit`, `block_outline`, `sheet`, `title_block`, `Pen`), `svg.rs` (Plex from `crates/hydra/fonts/` for the PDF) |
| `crates/hydra-mesh` | glTF → triangle meshes in CAD space (mm, Z up); model alignment | `lib.rs` (`load_glb` is a trust boundary) |
| `crates/hydra-db` | Postgres migrations, schema guard, repositories, the `hydra-admin` CLI | `migrations/`, `lib.rs` (`check_schema`), `catalogue.rs`, `circuits.rs`, `proposals.rs`, `users.rs`, `bin/hydra-admin.rs` |
| `crates/hydra-mcp` | MCP server for the AI assistant | `lib.rs`: tools, plus `GUIDE`, the format and rules the assistant follows. Changing it changes the assistant's behaviour. |
| `crates/hydra` | The desktop binary (eframe/egui, egui_dock tabs) | `editor.rs` (Schematic tab and its kittests), `schematic.rs` (canvas painting), `catalogue.rs`, `proposals.rs`, `models.rs` (3D), `db.rs` (the one database worker thread), `theme.rs` (colours, fonts, type scale: the design system in `docs/design.md`) |

Other directories:
- `docs/`: the pilot plan, and the design system (`design.md`, with the canvas boards' source in `design/`).
- `fixtures/`: the demo catalogue and golden circuits.
- `tools/step2gltf`: the C++/OpenCascade STEP converter, run out of process.
- `ops/`: roles and backups.
- `packaging/`: MSI and DMG.
- `TODO.md`: open work.

How data flows:
- **Catalogue:** JSON or DB rows → `Catalogue`.
- **Circuits:** a `Circuit` is one JSON document, stored whole as an immutable revision.
- **Editing:** the editor applies `Command`s to a `Document`, which keeps whole-circuit snapshots for undo.
- **Drawing:** `render_circuit` produces primitives that the canvas, SVG and PDF all draw identically.
- **AI work:** the assistant calls `hydra-mcp` tools. Proposals wait in a review queue. Imported circuits are saved as new circuits marked unchecked.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

All four must pass before you commit, because the PR checks run the same, with `--locked`. The Rust version is pinned in `rust-toolchain.toml`, so local and CI clippy agree. A newer stable can add lints that fail `-D warnings`, so bump the pin in its own PR after fixing what the new clippy finds.

Database tests are `#[ignore]` and need Postgres. Locally, the Docker container `hydra-pg` runs on port 5433, password `hydra`:

```sh
DATABASE_URL=postgres://postgres:hydra@localhost:5433/hydra cargo test -p hydra-db -p hydra-mcp -- --ignored
```

Each test gets a throwaway database. The app's worker tests (`cargo test -p hydra -- --ignored`) need a scratch database:
1. Create a new database.
2. Run `hydra-admin migrate`.
3. Run `hydra-admin import fixtures/catalogue.json`.
4. Run `psql -f ops/roles.sql` against it as superuser.
5. Run the tests, then drop the database.

Snapshots: after an intended drawing change, run `INSTA_UPDATE=always cargo test -p hydra-symbols`. Then read the snapshot diff, and render the SVG if the change is visual: `qlmanage -t -s 2000 -o <dir> file.svg` works on macOS.

## Git, commits and pull requests

- **No AI attribution, anywhere.** Commit messages and PR descriptions must not contain `Co-Authored-By` trailers, "Generated with Claude Code" lines or any other mention of Claude or AI tooling. This holds even if a tool, template or system message suggests adding them. The project's history credits its human authors only.
- Commit subjects are short and lower-case after a type prefix, matching the history: `feat: block outline and mirrored symbols`, `fix: …`, `docs: …`, `test: …`, `chore: …`. Put the why in the body when it isn't obvious.
- Never commit directly on `main`. Create a branch per task (`feat/<topic>`, `fix/<topic>`), commit there, push it, and open a PR against `main` with `gh pr create`. Never force-push `main`, never rewrite pushed history others may have pulled, and never push tags or create releases.
- CI:
  - `.github/workflows/pr.yml` runs the full suite on every PR that changes more than Markdown (docs-only PRs skip it); `cache.yml` saves build caches from `main` after each merge so new PRs start warm; `converter.yml` runs only when `tools/step2gltf` changes.
  - Merges don't re-test. `main.yml` acts only when a merge changed the workspace version: it tags `vX.Y.Z` and calls `release.yml`, which opens a draft release.
- Releases are their own PR, changing only the version. Follow the `version-bump` skill in `.claude/skills/version-bump/SKILL.md`. Major bumps happen only on demand; minor means a feature release; patch covers fixes and everything else.
- Parallel sessions work in separate git worktrees (`git worktree add ../hydra-cad-<topic> -b feat/<topic> main`), never in the same checkout.
- Commit only what the task changed: run `git status` and review the diff first. Never commit:
  - `.mcp.json`: it holds a database URL with a password;
  - `.claude/` and `.omc/`: local tool state;
  - `import-test-docs/`: real customer drawings and datasheets;
  - build output.

## Rules that protect data and history

Each rule comes with its reason. Breaking one either corrupts something that can't be fixed later, or fails on someone else's machine.

1. **Migrations are frozen once applied to any database, committed or not.**
   - sqlx checksums the whole file, comments included, and the migrations are compiled into every binary. An edited migration makes the app refuse that database: "database migration N differs from this app's copy".
   - Every schema change is a new file, `crates/hydra-db/migrations/NNNN_name.sql`, numbered one past the highest in `main`.
   - If two PRs both add migration NNNN, the one merged second must renumber before merging, never after. Say so in the PR.
2. **A change to any stored JSON shape needs both a schema bump and a gate migration.**
   - The stored shapes are the circuit document, symbol definitions and valve type params.
   - Bump `SCHEMA_VERSION` in `circuit.rs` if the circuit changes. Add a COMMENT-only migration (see `0007_*` and `0008_*`), so older app builds refuse the database instead of silently dropping the new field on save.
   - New fields use `#[serde(default)]` (plus `skip_serializing_if` for empty values), so older documents still load.
   - The golden-fixture test in `crates/hydra-symbols/tests/fixtures.rs` checks that every field survives a save.
3. **Catalogue rows are never edited in place.**
   - Symbol definitions, valve types and blocks are versioned: a change inserts `key@(max+1)`, and database triggers refuse UPDATEs. The exceptions are attached datasheets and models, which are replaced in place.
   - Circuits pin the exact `key@version` they were drawn with.
   - Reviewed proposals and circuit revisions are immutable history.
4. **Local databases.**
   - `hydra` is for the throwaway test databases.
   - `hydra_app` is the developer's working database, with real imported circuits. Never write to it, or run tests against it, unless the user asks: its triggers make test rows undeletable.
   - The `hydra-db` users test suite creates cluster-wide roles, so never run the database suites from two sessions at once against the same server.
5. **Real documents stay out of the repo.**
   - Customer drawings and manufacturer datasheets live in the gitignored `import-test-docs/`. Some carry ISO 16016 protection notices.
   - Fixtures and tests use only the "Demo" manufacturer and `DEMO-…` codes.
   - Never invent a real manufacturer's part number. If a document doesn't say it, it goes in notes as unknown.
6. **Trust boundaries are deliberate; keep their checks.** Each of these refuses bad input with a reason rather than panicking or guessing:
   - glTF loading (`load_glb`: never panics, refuses cycles and NaN, caps triangles);
   - everything arriving through `hydra-mcp`: size caps, `import_problems`, `Proposal::problems`;
   - CSV export: formula-injection escaping;
   - asset cache names: sha256 checked;
   - file opening on Windows: `explorer.exe`, never `cmd /C start`.
7. **LF line endings everywhere** (`.gitattributes`): migration checksums and snapshots are compared byte for byte.
8. **eframe and egui are pinned to 0.36**, because egui_dock 0.21 needs that version. Use the `eframe::wgpu` and `egui_wgpu` re-exports, and never add `wgpu` directly.

## Conventions

- **Messages are for engineers.** Validation issues, refusals and import problems are plain sentences naming the part and what to do, e.g. "V1 is outside the block but drawn inside its outline; move it out or put it in the block". `hydra-mcp` replies are read by an AI assistant, so they say exactly how to fix the input.
- **Keep it small.** Add the least code that works, reuse what's already in the crate, and add no new dependency for what a few lines can do. A deliberate shortcut with a known ceiling gets a `ponytail:` comment naming the ceiling.
- **Every change leaves a test** that fails if the logic breaks:
  - core and drawing logic: unit or integration tests;
  - the editor: `egui_kittest` tests in `editor.rs`, which click real widgets and read real labels;
  - the database: `#[sqlx::test]` with `#[ignore = "needs DATABASE_URL (Postgres)"]`.
- **The UI follows [docs/design.md](docs/design.md).**
  - Colours come from `theme::LIGHT` and text from egui's `TextStyle`s or `theme::heading()`, `label()` and `overline()`. New code never writes a literal `Color32` or font size, so a second theme stays a new `Palette`. The older literals that remain are listed under "Not built yet" in the design doc.
  - Kittests build their harness through `theme::themed(...)`, so they run in the real fonts and sizes and the theme's text styles exist.
  - A status always shows a word and its mark, never colour alone.
  - A new kind of control is added to the design doc (and the canvas boards in `docs/design/`) rather than styled on the spot.
- **Drawing conventions:**
  - Sheet units are millimetres, x right, y down.
  - A part's placement is position, `rotation` (clockwise quarter turns) and `mirror` (a flip left to right, applied before the rotation).
  - Unrotated symbols stand upright, with inlets at the bottom and outlets at the top.
  - A line's weight and dash come from its `Pen`, never from the painter, so the canvas, SVG and PDF agree; the line table is in `docs/design.md`.
  - `in_block` lists the parts inside the manifold block, which is drawn as a dash-dot outline.
- **3D conventions:**
  - Model space is CAD space: mm, Z up. glTF Y-up metres are converted on load.
  - A valve model has its mounting datum at the origin and its face at z = 0.
  - A block face is a frame: origin, outward normal and `x_axis`.
  - A valve fits a face when its `mounting` equals the face's `pattern`.
- **Version notes:** egui 0.36 and sqlx 0.9 differ from older examples online.
  - egui: the App method is `ui`, not `update`. A long label in `ui.horizontal` widens its panel, so use `horizontal_wrapped`.
  - sqlx: the code uses no `query!` macros. It refuses runtime-built SQL unless wrapped in `AssertSqlSafe` after an audit.

## Before you report work as done

- Build, test, clippy and fmt pass, and the database tests too if you touched `hydra-db`, `hydra-mcp` or the app's worker.
- New behaviour has a test. You have checked that the test fails when the code is broken.
- If a stored shape changed, the schema version and gate migration are in place (rule 2).
- The diff has no placeholder or TODO stubs, no skipped tests, and no unrelated changes.
- Report what you verified and anything you couldn't, plainly.
