# Hydra

Hydra is a desktop app for hydraulic circuits. You draw a circuit as an ISO 1219 schematic from a shared valve catalogue, view the valves in 3D, and keep everything in one Postgres database. It is written in Rust with egui and targets Windows 10/11 and macOS 15+.

Status: early development. The circuit model, symbol rendering, the schematic editor with revision history, the Postgres-backed catalogue (browse, search, edit, datasheets, JSON import), STEP to glTF conversion, a 3D view of manifold blocks with their valves mounted, and AI datasheet import through an MCP server with a review queue, SVG/PDF/BOM export, sign-in with per-user roles, an update check, installers and backups exist. The pilot comes next: see [docs/pilot.md](docs/pilot.md).

## Workspace

| Crate | What it holds |
| --- | --- |
| `crates/hydra-core` | Domain model: catalogue rows and manifold blocks with their mount faces, symbol definitions, circuit JSON (schema v4; v1 to v3 still read), validation, and the edit commands with undo/redo. No UI or database code. |
| `crates/hydra-symbols` | Turns symbol definitions into lines, circles and text, lays out whole circuits, and writes SVG. |
| `crates/hydra-mesh` | Loads glTF models into triangle meshes in Hydra's model space (millimetres, Z up) and runs the 3D smoke test. |
| `crates/hydra-db` | Postgres migrations (`migrations/`), the schema guard, the catalogue and circuit repositories, and the `hydra-admin` tool. |
| `crates/hydra-mcp` | The `hydra-mcp` MCP server: lets an AI assistant read datasheets and the catalogue and submit catalogue proposals for review. |
| `crates/hydra` | The `hydra` desktop binary: docked Schematic, Catalogue, Proposals and 3D tabs. |

`tools/step2gltf` is the STEP to glTF converter, a small C++ program on OpenCascade that runs outside the app (see its README).

`fixtures/` holds a sample catalogue, golden circuits, and in `fixtures/models/` demo STEP models with their converted glb files.

## Catalogue model

Symbol definitions and valve types are never edited in place. Every change adds a row with the same `key` and the next `version`, and circuits refer to one exact row as `key@version`, e.g. `vt-rpe3-06-z11@2`. Old circuits keep drawing with the version they were made with. Blocks are versioned the same way. Attaching a datasheet or a 3D model doesn't change how anything draws, so it changes the row in place, and a new version inherits both.

## 3D models and blocks

A block's mount faces are in the catalogue JSON: each has a name, a mounting pattern such as `ISO 4401-03`, and a frame in block coordinates (millimetres, Z up): `origin` at the pattern's datum, `normal` out of the block, and `x_axis` along the pattern. A valve model is drawn with its pattern datum at the origin, its mounting face at z = 0 and its body in +z, so a valve on a face is the face frame applied to the valve model. A valve fits a face when its valve type's `mounting` equals the face's pattern.

Screw-in cartridge cavities are faces too: the pattern is the cavity standard (e.g. `BP3 (C-10-3S)`), `origin` is on the seating plane and `normal` points out of the cavity. A cartridge model has its axis on +Z and its seating face at z = 0, so its thread goes into the block. Cartridge valve types have `"cavity": true` in their params and the cavity in `mounting`.

Models are stored in Postgres and attached with `hydra-admin`:

```sh
hydra-admin model block blk-demo@1 demo-block.step          # converts with step2gltf, keeps the STEP as source
hydra-admin model valve vt-rpe3-06-z11@1 valve.glb --source valve.step
hydra-admin model valve vt-sr1a-a2@1 StepFile_sr1a_a2.zip --preview sr1a.glb   # look before attaching
hydra-admin model valve vt-sr1a-a2@1 StepFile_sr1a_a2.zip --rotate x:180 --offset 0,0,-2
hydra-admin models                                           # 3D smoke test of every stored model
```

STEP files go through the converter named by `HYDRA_STEP2GLTF` (default `step2gltf` on the PATH); `--linear <mm>` sets the meshing tolerance. A manufacturer's zip with one STEP file in it works too. Every model is checked when attached and refused if it doesn't load or is empty or flat.

Manufacturers put a model's origin and axes anywhere. For a cartridge (STEP or zip, valve type with `"cavity": true`), `hydra-admin` guesses the alignment by default: the main axis on +Z, the wider end up, the seating shoulder at z = 0. Correct it with `--rotate x|y|z:<degrees>` (repeatable) and `--offset x,y,z` (mm), or turn it off with `--align none`; `--align auto` forces the guess for anything else. Everything else, and every `.glb`, is stored as given. `--preview out.glb` writes the aligned model and prints its size without attaching it, and the Models tab's "Preview a model file…" shows it on its own. The applied alignment is recorded in the model, and the original STEP is kept as its source.

## Datasheet import with an AI assistant

1. In the Catalogue tab, create a valve type without a symbol (a draft) and drop its PDF datasheet on the window.
2. Ask your AI assistant, with `hydra-mcp` connected, to fill in that valve type. It reads the datasheet and the symbol format, reuses or defines a symbol, and submits a proposal. Proposals are checked at once and problems go back to the assistant in plain words.
3. In the Proposals tab, compare the rendered symbol with the datasheet ("Open datasheet"), correct series, code, mounting or parameters if needed, then approve, or reject with a reason the assistant can read through `proposal_status`. Only approval writes catalogue rows: the next version of the valve type, its parts, and a new symbol if the proposal has one.

`hydra-mcp` runs on the engineer's machine over stdio and uses the same `HYDRA_DATABASE_URL` as the app, so proposals are recorded under the engineer's own Postgres role. Build it with `cargo build --release -p hydra-mcp`, then register it:

```sh
# Claude Code
claude mcp add hydra --scope user --env HYDRA_DATABASE_URL=postgres://user:password@server:5433/hydra -- /path/to/hydra-mcp
```

Claude Desktop: add to `claude_desktop_config.json` (macOS `~/Library/Application Support/Claude/`, Windows `%APPDATA%\Claude\`):

```json
{"mcpServers": {"hydra": {"command": "/path/to/hydra-mcp",
  "env": {"HYDRA_DATABASE_URL": "postgres://user:password@server:5433/hydra"}}}}
```

Any other MCP client works the same way. Its tools are `list_valve_types`, `get_valve_type`, `read_datasheet`, `symbol_format`, `submit_proposal`, `proposal_status` and `import_circuit`.

`read_datasheet` gives the assistant the datasheet's pages as images (two per call by default, up to four), with their text, so tables, symbols and ordering codes are read as printed whatever the layout. Pages are rendered in a separate `hydra-mcp` process with a time limit, so a broken PDF can't hang the server. Valve types record what they do (`function`), their ratings (`max_pressure_bar`, `max_flow_l_min`, `setting_range_bar`) and, for cartridges, their cavity; the reviewer can correct these in the Proposals tab.

**Update the app and `hydra-mcp` together.** This release adds symbol kinds (pilot-operated check, counterbalance, flow control, test point, hydraulic motor, pump, box) that older builds can't read, so migration 0007 makes older builds refuse to connect with "update the app". Likewise migration 0008: circuits now record which parts are inside the block, which older builds would drop on save. And migration 0009: circuits now record pilot and drain wires and plugged ports, and relief valves and flow controls can have a pilot port X.

## Circuit import with an AI assistant

1. Give the assistant photos or PDFs of a circuit drawing. A photo of a whole A3 sheet is too small to read once scaled down, so crop the parts table and the schematic into separate images.
2. Ask it to import the circuit. It reads the drawing, matches every part against the catalogue and calls `import_circuit`, which checks the circuit and saves it as a new circuit named after the drawing number, marked "imported by the AI assistant, unchecked". The tool replies with every warning, such as unconnected ports, for the assistant to double-check.
3. Open it in the Schematic tab, check it against the drawing and press **Mark checked** (any save does the same).

Every valve keeps what the drawing asked for (the code as printed) next to what the circuit uses:
- **red**: the valve isn't in the catalogue; it is drawn with a look-alike symbol;
- **amber**: the valve type is in the catalogue but not the exact part;
- **violet**: a temporary stand-in an engineer chose.

Select one to see catalogue valves that could stand in: the same function, the same mounting or cavity, and ratings that cover what the drawing asks for (a relief valve's setting must lie in the candidate's setting range). Pick one, confirm how its ports connect, and it is swapped in (undo works). When the requested valve is added to the catalogue later, the circuit says it is now available. Missing valves are warnings, never errors: the circuit saves and exports, and the PDF and the bill of materials say MISSING or STAND-IN in words. The assistant traces dashed lines as pilot or drain wires, drawn dashed even where they branch off a solid line of the same net (only traced wires can be dashed: a net drawn with automatic elbows is solid), marks plugged rail ends as plugged ports, and gives a relief valve piloted from another line, or a flow control with a second pilot on its compensator, a symbol with the extra pilot port X. Test points, fixed orifices, motors and pumps are drawn as generic parts, and parts-table rows such as the block, plugs and coils go into the bill of materials as extra lines. The editor edits these too: select a generic part to change its code, description and position, and open **Extra BOM lines** or **Import notes** above the sheet; each finished edit is one undo step.

The schematic draws the manifold block's outline as a dash-dot line around the parts inside it, with the block's ports on its edge and their names outside it. A part outside the block that stands in a corner holding no block part gets that corner cut out of the outline, so an L-shaped block draws as one; several parts in a corner step it, and a part standing at an edge snug between block parts gets a U-shaped cut in from that edge. Each part the outline is cut round is a warning in the editor, so a part left out of the block by mistake still shows. The drawing shows what is in the block and what isn't (motors, cylinders, pumps). Tick **In block** on a selected part to put it inside (a part added from the toolbar inside the outline goes in by itself); the assistant marks them from the drawing's own boundary. A part outside the block that stands inside the outline is a warning in the editor, and the import refuses it, so the drawing can't mislead. Where wires of two nets cross, the upright one hops over the other with a small arc, so crossing lines don't read as joined; labels, status words and port names move off the wires where there is room.

## Installing and configuring

The app reads one `hydra.ini` per machine, which only an administrator can change:

| | |
| --- | --- |
| Windows | `%ProgramData%\Hydra\hydra.ini` |
| macOS | `/Library/Application Support/Hydra/hydra.ini` |
| Linux | `/etc/hydra/hydra.ini` |

```ini
[hydra]
# The shared database, without user name or password.
database_url = postgres://hydra-db.office.local:5433/hydra?sslmode=require
# Where the app checks for a newer release on startup (optional).
update_url = https://example/hydra/latest.json
```

Lines are `key = value`; `#` and `;` start comments, section headers are ignored, and an empty value turns a key off. Use `sslmode=require` (or `verify-full`) so the connection is encrypted.

Each user signs in with their own Postgres role, so revisions and proposals record who made them. The sign-in window shows which database it is for; "Remember me on this computer" (on by default) keeps the password in the system keychain (macOS Keychain, Windows Credential Manager), one entry per database. "Sign out" in the status line forgets it; after a connection failure, "Retry" tries again with the same sign-in. A database superuser runs [ops/roles.sql](ops/roles.sql) once after migrating. It creates the group role `hydra_engineer` with what the app needs, and nothing more; `hydra_admin` for administrators, which adds no Postgres rights; and `hydra_manager`, which nobody signs in as. Then, still as the superuser, they add the first administrator, who adds everyone else in the Account tab, and run roles.sql again:

```sql
CREATE ROLE boss LOGIN PASSWORD '...' IN ROLE hydra_admin;
```

An administrator is a member of `hydra_admin` (a superuser counts as one too). Administrators manage users only through database functions that run as `hydra_manager` and check every change: the target must be a Hydra user (a member of `hydra_engineer` with no rights or memberships beyond that), never a superuser or any other role; administrators can't delete, disable, demote or sign out themselves, and Hydra always keeps one administrator who can sign in. Every administrator can manage every other one. Users who own database objects or hold privileges of their own can still be disabled, demoted and signed out, but only a superuser can set their password, promote, enable or delete them (after `REASSIGN OWNED BY alice TO ...`). Run roles.sql again after adding or changing users by hand (`CREATE ROLE alice LOGIN PASSWORD '...' IN ROLE hydra_engineer;`); on PostgreSQL 16 and later administrators can't change them until it has. roles.sql also takes back what its earlier version granted, when administrators had `CREATEROLE`, and removes `CREATEROLE` from every Hydra user, with a warning naming each.

The functions live in the schema `hydra_auth`, which roles.sql gives to `hydra_manager` with everything in it, so not even a database owner who isn't a superuser can change them. Migrations that change them must therefore run as a superuser, and roles.sql must run again after any migration that adds one. roles.sql refuses to run, and changes nothing, unless a superuser runs it. Keep the Hydra database itself owned by a superuser too: a database owner can change the database's search path, which the app's own queries rely on.

`HYDRA_DATABASE_URL`, a full URL with user name and password, overrides `hydra.ini` and skips the sign-in. Use it for development and CI.

The app's Account tab shows who is signed in and to which database, whether the password is remembered, and the Hydra version. Anyone can change their own password there; Hydra then updates the keychain and reconnects with it (through `HYDRA_DATABASE_URL`, update the variable yourself). Administrators also see the users, with their open sessions, and can add users, set passwords, grant or remove administrator rights, disable or enable sign-in, end sessions and delete users. Passwords reach the database only as SCRAM verifiers.

Installed (release) builds log to `%LOCALAPPDATA%\Hydra\hydra.log` on Windows, `~/Library/Logs/Hydra/hydra.log` on macOS and `$XDG_STATE_HOME/hydra/hydra.log` (default `~/.local/state/hydra/`) on Linux, emptied on each start.

## Run

Against the shared database:

```sh
export DATABASE_URL=postgres://user:password@server:5433/hydra
cargo run -p hydra-db --bin hydra-admin -- migrate                 # admin, once per release
cargo run -p hydra-db --bin hydra-admin -- import fixtures/catalogue.json
HYDRA_DATABASE_URL=$DATABASE_URL cargo run -p hydra
```

`hydra-admin` also has `check` (does the database match this build?), `attach <key@version> <file.pdf>`, and the model commands above.

With neither `HYDRA_DATABASE_URL` nor a `database_url`, a dev checkout runs offline and read-only, on `fixtures/catalogue.json`:

```sh
cargo run -p hydra                          # demo manifold
cargo run -p hydra -- my-circuit.json       # another circuit
cargo run -p hydra -- my-circuit.json my-catalogue.json
```

In the Catalogue tab, search by manufacturer, series, code or mounting, and select a row to see its symbol. "Edit as new version" saves a new version. Drop a PDF on the window to attach it to the selected valve type, or a catalogue JSON file to import it. Datasheets open in the system viewer and are cached locally by content hash.

In the Schematic tab:

- Place a valve type from the toolbar (or "Place in schematic" in the Catalogue tab), or add an external port such as P or A1.
- Drag a part to move it; it snaps to a 2.5 mm grid. R rotates the selected part, M mirrors it (e.g. a relief valve with its spring on the other side), Delete removes it.
- Drag from a valve port to another port to connect them; Shift-drag starts a connection from an external port. Right-click a port to disconnect it.
- Tick **Plugged** on an external port for the end of a rail drilled to the block's edge and closed with a plug; it is drawn as the ISO 1219 closed connection, a short bar across the line. Undo works.
- Nets are drawn with automatic elbows until you draw their wires. Press **Draw wires** (or W), click a port or a drawn wire to start, click each corner (segments run level or upright on the grid) and finish on a port or wire; a wire ending on another net, or on a loose port, joins it to the wire's net. Esc or right-click cancels the wire. Once a net has a drawn wire its elbows are gone, so the validation list warns about every port still to reach until the net is complete. Click a drawn wire to select it: its dropdown makes it a working line (solid) or a pilot or drain line (dashed, as drawings show them, even where it branches off a solid line of the same net), Delete removes it, **Draw net automatically** drops all of its net's wires. Joining nets, by a wire or by dragging, keeps both nets' drawn wires; a wire that joins two nets says so in the status line, and which name was dropped. Moving, rotating, mirroring, swapping or deleting a part, or disconnecting a port, drops the drawn wires of its nets; the status line names them, and Undo brings them back.
- Undo and redo with Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z (or the buttons). Validation problems are listed above the sheet.
- Drag the empty sheet to pan, scroll to zoom, double-click to fit.
- Save writes a new revision; nothing is overwritten. If someone else saved first, you're offered a reload or a save as copy. History lists every revision with a preview, and Restore makes an old revision the newest one, so a restore can itself be undone.
- Export writes the circuit as it is on screen, unsaved changes included, as SVG, PDF, or a bill of materials in CSV (one line per valve type version, part and status, then coded parts, the block and the extra lines, with designators, the part number used or the valve type's only one, and what the drawing requested).

In the 3D tab, choose the circuit's block and which valve sits on each face; only valves whose mounting pattern fits a face are offered. Putting a valve on a face also ticks it **In block**, so the schematic's block outline takes it in (one undo takes off both); taking it off a face, or unticking it, changes nothing else. Only the valves ticked **In block** in the schematic are expected on the block: one of them left off it is a warning, and so is a valve on the block that the schematic draws outside it. The view shows the block with its valves mounted, the valve selected in the schematic highlighted. Without a block it shows the selected valve alone. Drag to orbit, scroll to zoom, double-click to see everything. Models download once and are cached locally by hash; offline there are none.

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
DATABASE_URL=postgres://postgres:hydra@localhost:5433/hydra cargo test -p hydra-db -p hydra-mcp -- --ignored
```

Each of those tests gets a fresh database with all migrations applied. The app's database-worker tests need a migrated database with the fixture catalogue imported (see Run), and add a valve type and a circuit to it on each run:

```sh
DATABASE_URL=postgres://postgres:hydra@localhost:5433/hydra cargo test -p hydra -- --ignored
```

## Backups

The shared database is a single point of failure, so `ops/backup/` has a nightly `pg_dump` (plus roles) to another machine for Linux (systemd) and Windows Server (Task Scheduler), and a restore test that CI runs on every push. Setup, the monthly restore drill and the real restore procedure are in [ops/backup/README.md](ops/backup/README.md).

## CI

`.github/workflows/pr.yml` runs on every pull request into `main`:
- it checks formatting and runs clippy on Linux first; the Windows and macOS jobs wait for that and fail at once if it failed, so a lint error doesn't spend their (dearer) minutes;
- it runs the tests on Windows and macOS, and clippy again on Windows for its few Windows-only lines;
- it runs the database tests and the backup/restore test on Linux against a `postgres:15` service container, because GitHub only offers service containers on Linux runners;
- it checks that a changed version goes up.

A pull request that changes only Markdown files skips the builds and tests; their checks show as skipped, which counts as passed.

PR checks build without debug info, which links faster and keeps the caches small. A pull request can only read build caches saved on its own branch or on `main`, so `cache.yml` builds the same targets after every merge and saves them on `main`; a new pull request's first run then starts warm.

`converter.yml` builds the STEP converter only when `tools/step2gltf` changes.

The Rust version is pinned in `rust-toolchain.toml`, so CI and local builds run the same clippy.

Merges into `main` aren't tested again, so keep "Require branches to be up to date before merging" on in the branch protection: that way the merged code is exactly what the checks ran on. `main.yml` runs on each merge but only acts on a version bump (see below).

## Releasing and installing

### Cutting a release

1. Open a pull request that only changes `version` under `[workspace.package]` in the root `Cargo.toml` (and `Cargo.lock`), titled `chore: release vX.Y.Z`. Choose the version as follows:
   - major only when you decide to;
   - minor when features merged since the last release;
   - patch for fixes only.

   The `version-bump` skill (`.claude/skills/version-bump/SKILL.md`) has Claude Code prepare it with the release notes.
2. Merge it. `.github/workflows/main.yml` sees the new version, tags the merge commit `vX.Y.Z` and runs the release workflow. Pushing a tag by hand (`git tag v1.2.3 && git push origin v1.2.3`) starts it too; the tag must equal the workspace version or the release stops.
3. `.github/workflows/release.yml` builds the Windows MSI and the macOS DMG, a tools archive per platform (`hydra-mcp`, `hydra-admin`), signs what it can, and opens a **draft** GitHub release with all of them plus `latest.json`.
4. Review the draft, then publish it. Asset URLs, and so `latest.json`, only resolve once it's published.

Running the workflow by hand (Actions, Release, Run workflow) is an unsigned dry run: the files are uploaded as workflow artifacts and nothing is released. To build the macOS files locally run `packaging/macos/build-dmg.sh` (outputs in `target/package/dist`). The app icons are generated from `packaging/icon/hydra.svg` by `packaging/icon/build.sh`.

Repository secrets. Without them the builds still succeed, unsigned, with a warning:

| Secret | What it is |
| --- | --- |
| `WINDOWS_CERT_PFX_BASE64` | Code-signing certificate with its private key, as a base64-encoded `.pfx` |
| `WINDOWS_CERT_PASSWORD` | Password of that `.pfx` |
| `MACOS_CERT_P12_BASE64` | "Developer ID Application" certificate with its private key, as a base64-encoded `.p12` (export it from Keychain Access) |
| `MACOS_CERT_PASSWORD` | Password of that `.p12` |
| `APPLE_ID` | Apple ID email of a member of the Apple Developer team, for notarization |
| `APPLE_TEAM_ID` | The 10-character Apple Developer team ID |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple ID (appleid.apple.com) |

Base64-encode a certificate with `base64 -i cert.p12 | pbcopy` (macOS) or `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))` (PowerShell).

### Installing on Windows 10/11

The MSI installs per machine to `C:\Program Files\Hydra` and adds a Start menu shortcut. An administrator sets the database and update addresses at install time; they are written to `C:\ProgramData\Hydra\hydra.ini` and kept across upgrades and uninstalls (installing again without them leaves the file alone):

```bat
msiexec /i Hydra-1.2.3-x64.msi DATABASE_URL="postgres://db.example:5433/hydra?sslmode=require" UPDATE_URL="https://example/hydra/latest.json"
```

Add `/qn` for a silent install from Group Policy, Intune or another deployment tool. `DATABASE_URL` holds no user name or password. To change the addresses later, edit `hydra.ini` or push a new one. The tools zip holds `hydra-mcp.exe` and `hydra-admin.exe`, to unpack wherever suits.

### Installing on macOS 15+

Open the DMG and drag Hydra to Applications. The machine configuration lives in `/Library/Application Support/Hydra/hydra.ini`; deploy it with your MDM or by hand:

```sh
sudo mkdir -p "/Library/Application Support/Hydra"
sudo tee "/Library/Application Support/Hydra/hydra.ini" <<'INI'
[hydra]
database_url = postgres://db.example:5433/hydra?sslmode=require
update_url = https://example/hydra/latest.json
INI
```

The tools zip holds universal `hydra-mcp` and `hydra-admin` binaries.

### Updates

On startup the app fetches `update_url`, a `latest.json` such as `{"version": "1.2.3", "windows": "<msi url>", "macos": "<dmg url>", "notes": "…"}`, and offers the newer installer. Each release carries its own `latest.json`, so `https://github.com/thinato/hydra-cad/releases/latest/download/latest.json` always points at the newest published release, but only if users can reach the repository without signing in. For a private repository, host `latest.json` and the installers somewhere they can reach (an internal https server) and set `update_url` to that.

The STEP-to-glTF converter (`tools/step2gltf`) is not packaged yet; build it with CMake as in CI.
