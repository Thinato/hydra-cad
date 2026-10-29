# Hydra

Hydra is a desktop app for hydraulic circuits. You draw a circuit as an ISO 1219 schematic from a shared valve catalogue, view the valves in 3D, and keep everything in one Postgres database. It is written in Rust with egui and targets Windows 10/11 and macOS 15+.

Status: early development. The circuit model, symbol rendering, the schematic editor with revision history, the Postgres-backed catalogue (browse, search, edit, datasheets, JSON import), STEP to glTF conversion and a 3D view of manifold blocks with their valves mounted exist. AI datasheet import and export are still to come.

## Workspace

| Crate | What it holds |
| --- | --- |
| `crates/hydra-core` | Domain model: catalogue rows and manifold blocks with their mount faces, symbol definitions, circuit JSON (schema v2; v1 still reads), validation, and the edit commands with undo/redo. No UI or database code. |
| `crates/hydra-symbols` | Turns symbol definitions into lines, circles and text, lays out whole circuits, and writes SVG. |
| `crates/hydra-mesh` | Loads glTF models into triangle meshes in Hydra's model space (millimetres, Z up) and runs the 3D smoke test. |
| `crates/hydra-db` | Postgres migrations (`migrations/`), the schema guard, the catalogue and circuit repositories, and the `hydra-admin` tool. |
| `crates/hydra` | The `hydra` desktop binary: docked Schematic, Catalogue and 3D tabs. |

`tools/step2gltf` is the STEP to glTF converter, a small C++ program on OpenCascade that runs outside the app (see its README).

`fixtures/` holds a sample catalogue, golden circuits, and in `fixtures/models/` demo STEP models with their converted glb files.

## Catalogue model

Symbol definitions and valve types are never edited in place. Every change adds a row with the same `key` and the next `version`, and circuits refer to one exact row as `key@version`, e.g. `vt-rpe3-06-z11@2`. Old circuits keep drawing with the version they were made with. Blocks are versioned the same way. Attaching a datasheet or a 3D model doesn't change how anything draws, so it changes the row in place, and a new version inherits both.

## 3D models and blocks

A block's mount faces are in the catalogue JSON: each has a name, a mounting pattern such as `ISO 4401-03`, and a frame in block coordinates (millimetres, Z up): `origin` at the pattern's datum, `normal` out of the block, and `x_axis` along the pattern. A valve model is drawn with its pattern datum at the origin, its mounting face at z = 0 and its body in +z, so a valve on a face is the face frame applied to the valve model. A valve fits a face when its valve type's `mounting` equals the face's pattern.

Models are stored in Postgres and attached with `hydra-admin`:

```sh
hydra-admin model block blk-demo@1 demo-block.step          # converts with step2gltf, keeps the STEP as source
hydra-admin model valve vt-rpe3-06-z11@1 valve.glb --source valve.step
hydra-admin models                                           # 3D smoke test of every stored model
```

STEP files go through the converter named by `HYDRA_STEP2GLTF` (default `step2gltf` on the PATH); `--linear <mm>` sets the meshing tolerance. Every model is checked when attached and refused if it doesn't load or is empty or flat.

## Run

Against the shared database:

```sh
export DATABASE_URL=postgres://user:password@server:5433/hydra
cargo run -p hydra-db --bin hydra-admin -- migrate                 # admin, once per release
cargo run -p hydra-db --bin hydra-admin -- import fixtures/catalogue.json
HYDRA_DATABASE_URL=$DATABASE_URL cargo run -p hydra
```

`hydra-admin` also has `check` (does the database match this build?), `attach <key@version> <file.pdf>`, and the model commands above.

Without `HYDRA_DATABASE_URL` the app runs offline and read-only, on `fixtures/catalogue.json`:

```sh
cargo run -p hydra                          # demo manifold
cargo run -p hydra -- my-circuit.json       # another circuit
cargo run -p hydra -- my-circuit.json my-catalogue.json
```

In the Catalogue tab, search by manufacturer, series, code or mounting, and select a row to see its symbol. "Edit as new version" saves a new version. Drop a PDF on the window to attach it to the selected valve type, or a catalogue JSON file to import it. Datasheets open in the system viewer and are cached locally by content hash.

In the Schematic tab:

- Place a valve type from the toolbar (or "Place in schematic" in the Catalogue tab), or add an external port such as P or A1.
- Drag a part to move it; it snaps to a 2.5 mm grid. R rotates the selected part, Delete removes it.
- Drag from a valve port to another port to connect them; Shift-drag starts a connection from an external port. Right-click a port to disconnect it.
- Undo and redo with Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z (or the buttons). Validation problems are listed above the sheet.
- Drag the empty sheet to pan, scroll to zoom, double-click to fit.
- Save writes a new revision; nothing is overwritten. If someone else saved first, you're offered a reload or a save as copy. History lists every revision with a preview, and Restore makes an old revision the newest one, so a restore can itself be undone.

In the 3D tab, choose the circuit's block and which valve sits on each face; only valves whose mounting pattern fits a face are offered. The view shows the block with its valves mounted, the valve selected in the schematic highlighted. Without a block it shows the selected valve alone. Drag to orbit, scroll to zoom, double-click to see everything. Models download once and are cached locally by hash; offline there are none.

## Test

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Symbol and circuit drawings are checked against SVG snapshots in `crates/hydra-symbols/tests/snapshots`. After an intended drawing change, run `INSTA_UPDATE=always cargo test -p hydra-symbols`, then review the snapshot diff before committing it.

### Database tests

The database tests need a Postgres server, so they are marked `#[ignore]`. Start one locally with Docker:

```sh
docker run -d --name hydra-pg -e POSTGRES_PASSWORD=hydra -e POSTGRES_DB=hydra -p 5433:5432 postgres:15
DATABASE_URL=postgres://postgres:hydra@localhost:5433/hydra cargo test -p hydra-db -- --ignored
```

Each of those tests gets a fresh database with all migrations applied. The app's database-worker tests need a migrated database with the fixture catalogue imported (see Run), and add a valve type and a circuit to it on each run:

```sh
DATABASE_URL=postgres://postgres:hydra@localhost:5433/hydra cargo test -p hydra -- --ignored
```

## CI

`.github/workflows/ci.yml` checks formatting, runs clippy, builds and tests on Windows and macOS. It runs the database tests on Linux against a `postgres:15` service container, because GitHub only offers service containers on Linux runners.
