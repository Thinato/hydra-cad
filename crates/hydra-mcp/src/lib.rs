//! MCP tools that let an AI assistant fill in draft valve types from their datasheets.
//!
//! The assistant reads the catalogue and the attached PDF, and submits a [`Proposal`]. Every
//! proposal is checked with [`Proposal::problems`]; valid ones wait in Hydra's review queue, and
//! only an engineer's approval writes catalogue rows.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use hydra_core::{
    Catalogue, Circuit, CircuitStatus, Import, Proposal, SCHEMA_VERSION, Severity, ValveStatus,
    ValveType,
};
use hydra_db::{LoadedCatalogue, PgPool, ProposalStatus};
use hydra_symbols::{notched_parts, outline_issues, wire_problems, wire_warnings};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Sent at initialize: the workflow, in a few lines.
pub const INSTRUCTIONS: &str = "\
Hydra is a hydraulic CAD app. With these tools you fill in draft valve types from their \
manufacturer datasheets, and turn circuit drawings into Hydra circuits. An engineer reviews \
every proposal in Hydra before anything enters the catalogue, and checks every imported circuit.
Datasheets:
1. list_valve_types: find the draft the engineer means (symbol: none, datasheet: yes).
2. read_datasheet: it returns the PDF's pages as images, plus their text. Read the images: \
tables, symbols and ordering codes are laid out there.
3. symbol_format: read the symbol and proposal format and the existing symbol definitions.
4. Reuse an existing symbol definition that shows the same valve, or define a new one.
5. submit_proposal: it is checked at once; fix every problem it reports and submit again.
6. Tell the engineer the proposal id and to review it in Hydra's Proposals tab.
Circuit drawings (photos or PDFs the engineer gives you, ideally the parts table and the \
schematic as separate images):
1. symbol_format: read the circuit import section and the symbols; list_valve_types: the \
catalogue to match the parts table against.
2. import_circuit with every part, connection and parts-table row; fix the problems it reports \
and import again; check each warning it returns against the drawing.
3. Tell the engineer to open the circuit in Hydra and check it.
Never invent part numbers or values that aren't in the documents; put doubts in the notes.";

/// The `symbol_format` guide, before the list of existing symbol definitions.
const GUIDE: &str = r#"# Symbol definitions

A symbol definition says what an ISO 1219 valve symbol shows; Hydra draws it. It is JSON tagged by "kind":

{"kind": "check", "spring": true}         non-return valve, free flow A -> B; spring-loaded if spring
{"kind": "orifice", "adjustable": false}  restriction between A and B; a throttle if adjustable
{"kind": "relief", "adjustable": true}    pressure relief valve from P to T
{"kind": "pilot_check", "spring": true}   pilot-operated check A -> B, opened backwards by pilot X
{"kind": "counterbalance"}                counterbalance / overcenter valve: V (valve side), C (load), pilot X
{"kind": "flow_control", "actuator": "proportional", "compensated": true, "bypass": true}
                                          flow control In -> Out; actuator "manual" or "proportional";
                                          compensated = pressure compensated; bypass adds port Bypass
{"kind": "directional_valve", ...}        spool or poppet valve, see below

These are valve symbols: the only kinds a catalogue valve type may use. Their ports are fixed:
check and orifice A, B; relief P, T; pilot_check A, B, X; counterbalance V, C, X; flow_control
In, Out (and Bypass). Datasheets often number cartridge ports 1, 2, 3: write in the notes which
number is which port (e.g. "port 1 = V, 2 = C, 3 = X, page 1").

Circuits can also draw parts that are not catalogue valves:
{"kind": "test_point"}                    gauge connection, port M
{"kind": "hydraulic_motor", "bidirectional": false, "variable": false, "drain": false}
                                          ports A, B (and L when drain)
{"kind": "pump", "variable": false}       ports S (suction), P (pressure)
{"kind": "box", "label": "PB10", "ports": ["1", "2", "3"]}
                                          a labelled box, for anything without a symbol of its own

A 2/2 poppet (solenoid cartridge) is a directional_valve with two ports and two positions.

directional_valve fields:
- ports: [{"name": "A", "side": "top"}, ...] in drawing order along their side ("top" or "bottom"), usually A, B on top and P, T at the bottom.
- positions: the switching positions (envelopes), left to right. Each has:
  - flows: [["P", "A"], ...] paths drawn as arrows, [from, to]
  - links: [["P", "T"]] connections drawn as plain lines, without direction
  - blocked: ["A", "B"] ports closed in this position
  Ports a position doesn't mention are drawn open-ended. Every port named must be in ports.
- normal_position: 0-based index of the rest position, the one the ports connect to (1 = centre of a 3-position valve).
- left, right: actuators on each end, from "solenoid", "spring", "lever", "pilot", "detent", e.g. ["solenoid", "spring"]. Operating the left actuator brings the left position into effect, and likewise on the right.

# Proposal

Submit with submit_proposal:

{
  "valve_type": {
    "key": "vt-demo-10", "version": 1, "manufacturer": "demo",
    "series": "DEMO-10", "code": "4-3-C", "symbol": "sym-4-3-closed-centre@1",
    "mounting": "ISO 4401-05",
    "params": {"function": "directional", "max_pressure_bar": 315, "max_flow_l_min": 120}
  },
  "new_symbol": {"key": "sym-...", "version": 1, "definition": {...}},
  "parts": [{"part_number": "...", "options": {"voltage": "24 V DC", "connector": "..."}}],
  "notes": "..."
}

Rules:
- valve_type.version is the base: the latest version of the key you update (list_valve_types, get_valve_type), or 0 for a new key. Approval saves key@(version + 1).
- manufacturer is a manufacturer key from list_valve_types. series and code must not be empty. mounting is the subplate pattern (e.g. "ISO 4401-03") or, for a screw-in cartridge, the cavity as printed (e.g. "BP3 (C-10-3S)"); leave it out if there is neither. When the catalogue already spells a standard (list_valve_types, block faces), copy its spelling exactly.
- params is a JSON object of datasheet values, with units in the names. These keys are checked, because stand-in suggestions rely on them:
  - function (required for a new key): one of directional, check, pilot_operated_check, relief, counterbalance, flow_control, pressure_reducing, sequence, other.
  - max_pressure_bar, max_flow_l_min: plain numbers, no units.
  - setting_range_bar: [min, max] a pressure valve can be set to. When spring variants differ, put it in each part's options instead.
  - cavity: true for a screw-in cartridge (then mounting names the cavity).
- symbol is "key@version". Reuse an existing definition below when it shows exactly this valve. Otherwise add new_symbol (leave it out when reusing): its version is the next of its key (1 for a new key), and valve_type.symbol must be new_symbol's key@version.
- parts are the orderable part numbers of the new version, with their ordering-code options. Parts of the base version are kept, unless you list the same part number again to replace its options. Don't list a part twice.
- notes cite the datasheet page of every value (e.g. "pressure and flow: page 1; part numbers: page 2") and say anything the reviewer should check.
- Never invent part numbers or values that aren't in the datasheet; put doubts in notes.

# Functions, ratings, cartridges, ordering codes

- Parts are only the part numbers the datasheet prints, plus any a circuit asks for. Check a requested code against the ordering code and cite the page. Never expand an ordering code into every combination.
- To add a part a circuit asks for to an existing valve type, propose valve_type.version = its latest version, with just that part: the existing parts are kept.
- A datasheet covering several sizes (e.g. sizes 16 and 25) is one valve type per size, so one proposal per type. The engineer creates one draft per size and attaches the same PDF to each; each proposal's notes cite the pages for its size.

# Circuit import

Send a circuit read from a drawing with import_circuit:

{
  "name": "MH-450",
  "components": [
    {"id": "P", "kind": "port", "label": "P"},
    {"id": "7.1", "kind": "valve", "valve_type": "vt-demo-cartridge-relief@1", "part": "DEMO-CR10-210",
     "requested": {"code": "DEMO-CR10-210", "description": "Relief valve 210 bar", "function": "relief",
                   "mounting": "DEMO-C10-2", "setting_bar": 210}},
    {"id": "5.2", "kind": "valve",
     "requested": {"code": "...", "description": "...", "function": "directional", "mounting": "ISO 4401-05"},
     "display": {"kind": "directional_valve", ...}},
    {"id": "X1", "kind": "item", "symbol": {"kind": "orifice"}, "label": "X 0.5 mm"},
    {"id": "M1", "kind": "item", "symbol": {"kind": "test_point"}, "label": "MP SAE-04",
     "code": "...", "description": "Test point", "position": "14"}
  ],
  "nets": [{"id": "n1", "ports": [{"component": "P", "port": "port"}, {"component": "7.1", "port": "P"}]}],
  "layout": {"components": {"P": {"x": 0, "y": 100}, "7.1": {"x": 40, "y": 60, "rotation": 0}},
             "wires": {"n1": [[[0, 100], [0, 84], [40, 84], [40, 68]]]}},
  "in_block": ["P", "7.1", "5.2", "X1", "M1"],
  "bom_extra": [{"position": "1", "quantity": 1, "code": "...", "description": "Manifold block"}],
  "import": {"notes": ["row 12 of the parts table is illegible"]}
}

Rules:
- name is the drawing number. Every parts-table row ends up exactly once: as a valve, an item or a bom_extra line.
- Valves: "requested" holds the code exactly as printed, the description, the function and the mounting or cavity. If the catalogue has that valve type, set valve_type (key@version) and, when the exact part is listed, part. If not, leave valve_type out, set "display" to a look-alike symbol definition (copy an existing definition's JSON, or a box with the valve's ports), and fill requested.function and requested.mounting; add requested.key when you propose the valve type. setting_bar, max_pressure_bar and max_flow_l_min go in requested when the drawing states them.
- Fixed orifices, test points, hydraulic motors and pumps are items, never valves; give them code, description and position when the parts table lists them. The block, plugs, coils and seals are bom_extra lines.
- A part with no symbol of its own (e.g. a 3-port pressure compensator) is a valve missing from the catalogue whose display is a labelled box with its ports.
- External connections (P, T, A1, MP...) are ports; their single port is "port".
- Every connection on the schematic is a net of component ports. Port names are the symbol's (see above). Draw the layout in millimetres, x right and y down, as on the drawing: keep every part in the same place relative to the others (same row, same column, same order along a line), with about 30 mm between parts so their labels don't overlap.
- Every symbol must look exactly as it does on the drawing: the engineer compares the two side by side. Place it with "rotation" (0, 90, 180 or 270 degrees clockwise) and "mirror" (true flips it left to right, before the rotation). Unrotated and unmirrored, symbols stand upright with flow running up: inlets (P, A, In, the motor's A) at the bottom, outlets (T, B, Out) at the top, a relief valve's spring on the right and its pilot line on the left, a check valve's spring on top, a test point's M at the bottom; directional valves show their positions side by side, ports above and below, actuators at the ends as listed in left/right. 90 puts the bottom port on the left, 180 at the top, 270 on the right.
- For each symbol, find the one of the eight placements (four rotations, each with or without mirror) where both of these match the drawing: (1) which side every port leaves the symbol, and (2) which side the spring, solenoid, lever, adjustment arrow, pilot line and drain are on. Ports alone are not enough. Examples for a relief valve: P bottom, T top, spring right: rotation 0. Spring left instead: mirror. P left, T right, spring below: 90. P left, T right, spring on top: 90 with mirror. P right, T left, spring on top: 270. P right, T left, spring below: 270 with mirror. A check valve whose flow runs downwards is 180. If no placement matches, say which symbol and how it differs in import.notes.
- layout.wires traces each net's lines as the drawing routes them: per net id, a list of wires, each a list of [x, y] points in the same millimetres. A wire runs horizontally or vertically from point to point; where the line turns, add the corner point. Each wire ends on a port of its net or on another wire of the same net (a T-junction, drawn with a dot), and together a net's wires reach every port of the net in one connected piece: wires that only cross are not joined, so end one on the other. A wire never touches a port or wire of another net, except to cross it, and never runs through a part's symbol. Keep the drawing's corners, junctions and crossings, and the side of each part a line passes. Trace every net: a net left out of wires is drawn with automatic elbows, which rarely match the drawing. Hydra works out where each port is from the symbol and its placement; when a wire end misses a port, the problem says where the net's ports are, so move the end, and the corner before it, there.
- in_block lists every component inside the manifold block: what the drawing's dash-dot boundary encloses, with the ports on its edge. Parts outside it (motors, cylinders, pumps, valves mounted elsewhere) are left out, and their layout places them more than 4 mm outside the area the block's parts span, since Hydra draws the block's outline around everything in in_block. For an L-shaped, U-shaped or stepped block, an outside part may instead stand in a corner holding no block part, or at the block's edge snug between block parts (nearer the edge than its own size, and less than 12 mm from the block parts beside it on both sides along the edge): Hydra cuts the outline in round it, and lists each such part in the reply's warnings to check. Leave in_block out when the drawing shows no block.
- What you can't read goes in import.notes, never guessed.

# Existing symbol definitions
"#;

/// Datasheet text returned in one call when there are no page images, in characters.
const MAX_CHARS: usize = 60_000;
/// Datasheet text returned beside the page images, in characters.
const MAX_CHARS_WITH_IMAGES: usize = 15_000;
/// Largest datasheet read; manufacturer datasheets are a few MB.
pub const MAX_PDF_BYTES: usize = 64 << 20;
/// Long edge of a page image in pixels: the most Claude looks at without scaling down.
pub const MAX_PX: u32 = 1568;
/// Page images per call: by default, and at most.
const DEFAULT_PAGES: u32 = 2;
const MAX_PAGES: u32 = 4;
/// Version of the `hydra-mcp render-pdf` arguments and output.
pub const RENDER_PROTOCOL: &str = "1";
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest renderer output read: four pages are well under 1 MB.
const MAX_RENDER_OUTPUT: usize = 32 << 20;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Only valve types whose key, manufacturer, series, code or mounting contains this text.
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeyParams {
    /// Valve type key, without @version, e.g. "vt-demo-10".
    pub key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DatasheetParams {
    /// Valve type key, without @version.
    pub key: String,
    /// First page to read, from 1. Default 1.
    #[serde(default)]
    pub first_page: Option<u32>,
    /// Last page to read. Default: as many as max_pages allows.
    #[serde(default)]
    pub last_page: Option<u32>,
    /// Pages to show as images in this call, 1 to 4. Default 2.
    #[serde(default)]
    pub max_pages: Option<u32>,
}

/// What `hydra-mcp render-pdf` prints: the page count, the text of every page, and the
/// requested pages as base64 PNGs.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Rendered {
    pub pages: u32,
    pub text: Option<Vec<String>>,
    pub text_error: Option<String>,
    pub images: Vec<PageImage>,
    pub image_errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PageImage {
    /// From 1.
    pub page: u32,
    pub png_base64: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposalParams {
    /// The proposal: a JSON object with valve_type, optional new_symbol, parts and notes, as
    /// symbol_format describes.
    pub proposal: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CircuitParams {
    /// The circuit read from a drawing: a JSON object as symbol_format's "Circuit import"
    /// section describes.
    pub circuit: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdParams {
    /// Proposal id, as submit_proposal returned it.
    pub id: String,
}

/// The MCP server: one database pool, the engineer's own role, and the `hydra-mcp` program
/// that renders datasheets.
#[derive(Clone)]
pub struct HydraMcp {
    pool: PgPool,
    renderer: PathBuf,
}

#[tool_router]
impl HydraMcp {
    pub fn new(pool: PgPool, renderer: PathBuf) -> Self {
        Self { pool, renderer }
    }

    #[tool(
        description = "List the manufacturers and every valve type (latest version), showing \
        which are drafts without a symbol and which have a datasheet attached. Optional search \
        filters by key, manufacturer, series, code or mounting."
    )]
    pub async fn list_valve_types(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<String, String> {
        let loaded = self.load().await?;
        let c = &loaded.catalogue;
        let mut out = String::from("Manufacturers:\n");
        for m in &c.manufacturers {
            out += &format!("  {}  {}\n", m.key, m.name);
        }
        let search = p.search.unwrap_or_default().to_lowercase();
        let rows: Vec<_> = c
            .latest_valve_types()
            .into_values()
            .filter(|v| {
                let maker = c.manufacturers.iter().find(|m| m.key == v.manufacturer);
                [
                    Some(v.key.as_str()),
                    Some(&v.manufacturer),
                    maker.map(|m| m.name.as_str()),
                    Some(&v.series),
                    Some(&v.code),
                    v.mounting.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|s| s.to_lowercase().contains(&search))
            })
            .collect();
        out += "\nValve types (key@latest version  manufacturer  series code  mounting  function  symbol  datasheet):\n";
        if rows.is_empty() {
            out += "  none match\n";
        }
        for v in rows {
            let r = v.reference();
            let datasheet = loaded.datasheets.contains_key(&r);
            let symbol = v
                .symbol
                .as_ref()
                .map_or("none (draft)".to_string(), |s| s.to_string());
            let function = c.function_of(&r).map_or("-", |f| f.name());
            out += &format!(
                "  {r}  {}  {} {}  {}{}  {function}  symbol: {symbol}  datasheet: {}",
                v.manufacturer,
                v.series,
                v.code,
                v.mounting.as_deref().unwrap_or("-"),
                if v.is_cartridge() { " (cavity)" } else { "" },
                if datasheet { "yes" } else { "no" }
            );
            if v.symbol.is_none() && datasheet {
                out += "  <- draft with a datasheet, ready to fill in";
            }
            out += "\n";
        }
        Ok(out)
    }

    #[tool(
        description = "One valve type as JSON: its latest version, that version's symbol \
        definition and parts, all its versions, and whether a datasheet is attached."
    )]
    pub async fn get_valve_type(
        &self,
        Parameters(p): Parameters<KeyParams>,
    ) -> Result<String, String> {
        let loaded = self.load().await?;
        let c = &loaded.catalogue;
        let latest = latest_of(c, &p.key)?;
        let r = latest.reference();
        let symbol = latest
            .symbol
            .as_ref()
            .map(|s| serde_json::json!({ "ref": s, "definition": c.symbol_def(s) }));
        let json = serde_json::json!({
            "latest": latest,
            "symbol": symbol,
            "parts": c.parts_of(&r).collect::<Vec<_>>(),
            "versions": c.valve_types.iter().filter(|v| v.key == p.key).collect::<Vec<_>>(),
            "datasheet_attached": loaded.datasheets.contains_key(&r),
        });
        Ok(serde_json::to_string_pretty(&json).expect("catalogue rows serialize"))
    }

    #[tool(
        description = "The PDF datasheet attached to a valve type's latest version: its text \
        with --- page N --- markers, then an image of each page. Two pages per call by default \
        (max_pages up to 4); pass first_page to read on."
    )]
    pub async fn read_datasheet(
        &self,
        Parameters(p): Parameters<DatasheetParams>,
    ) -> Result<CallToolResult, String> {
        let loaded = self.load().await?;
        let r = latest_of(&loaded.catalogue, &p.key)?.reference();
        let Some(sha) = loaded.datasheets.get(&r) else {
            return Err(format!(
                "valve type {r} has no datasheet attached; ask the engineer to attach the PDF \
                 to it in Hydra's Catalogue tab"
            ));
        };
        let bytes = hydra_db::asset_bytes(&self.pool, sha)
            .await
            .map_err(db_error)?
            .filter(|b| hydra_db::sha256_hex(b) == *sha)
            .ok_or_else(|| {
                format!(
                    "the datasheet of {r} is missing or damaged in the database; ask the \
                     engineer to attach the PDF again"
                )
            })?;
        read_pdf(&self.renderer, &r.to_string(), bytes, &p).await
    }

    #[tool(
        description = "How to write a symbol definition and a proposal, with every existing \
        symbol definition to reuse. Read it before submit_proposal."
    )]
    pub async fn symbol_format(&self) -> Result<String, String> {
        Ok(symbol_guide(&self.load().await?.catalogue))
    }

    #[tool(
        description = "Submit a proposal (JSON, shape in symbol_format) for the engineer to \
        review in Hydra. It is checked first; if it has problems they are all listed, so you \
        can fix them and submit again."
    )]
    pub async fn submit_proposal(
        &self,
        Parameters(p): Parameters<ProposalParams>,
    ) -> Result<String, String> {
        let proposal = parse_proposal(p.proposal)?;
        match hydra_db::submit_proposal(&self.pool, &proposal).await {
            Ok(id) => Ok(format!(
                "Proposal {id} for {} is waiting for review in Hydra's Proposals tab.",
                proposal.target()
            )),
            Err(hydra_db::Error::Invalid(problems)) => Err(format!(
                "The proposal was not accepted. Fix these problems and submit again:\n- {}",
                problems.join("\n- ")
            )),
            Err(hydra_db::Error::AlreadyPending { id }) => Err(format!(
                "The proposal was not accepted: proposal {id} for {} is still waiting for \
                 review, and only one of them could be approved. Don't submit it again. Tell \
                 the engineer to approve or reject proposal {id} in Hydra's Proposals tab; if \
                 they reject it, read their note with proposal_status and submit a corrected \
                 proposal.",
                proposal.valve_type.reference()
            )),
            Err(e) => Err(db_error(e)),
        }
    }

    #[tool(
        description = "Save a circuit read from a drawing (JSON, shape in symbol_format's \
        \"Circuit import\" section) as a new circuit in Hydra, marked as imported and \
        unchecked. It is checked first; if it has problems they are all listed, so you can fix \
        them and import again. The reply lists every warning to double-check against the drawing."
    )]
    pub async fn import_circuit(
        &self,
        Parameters(p): Parameters<CircuitParams>,
    ) -> Result<String, String> {
        let mut value = json_object(p.circuit, "circuit")?;
        // Whatever version the assistant wrote, it wrote this build's format.
        value["schema"] = SCHEMA_VERSION.into();
        let mut circuit: Circuit = parse_json(value, "circuit")?;
        circuit.import.get_or_insert_with(Import::default);
        let catalogue = self.load().await?.catalogue;
        circuit.choose_requested_parts(&catalogue);
        let mut problems = circuit.import_problems(&catalogue);
        // The assistant drew the layout, so a part shown in the wrong place is its to fix.
        problems.extend(outline_issues(&circuit, &catalogue).iter().map(|i| match i {
            hydra_core::Issue::DrawnInsideBlock(id) => format!(
                "{id} is not in in_block, but its layout puts it over the block's outline; place it \
                 more than 4 mm outside the area the block's parts span, in a corner holding no \
                 block part, or at the block's edge snug between block parts, less than 12 mm \
                 from the block parts beside it on both sides (change in_block only if the \
                 drawing shows it inside)"
            ),
            i => i.to_string(),
        }));
        problems.extend(wire_problems(&circuit, &catalogue));
        if !problems.is_empty() {
            tracing::warn!(
                name = circuit.name,
                components = circuit.components.len(),
                problems = problems.len(),
                "import_circuit refused"
            );
            return Err(format!(
                "The circuit was not saved. Fix these problems and import again:\n- {}",
                problems.join("\n- ")
            ));
        }
        let message = format!(
            "Imported by the AI assistant from drawing {}; unchecked",
            circuit.name
        );
        let opened = match hydra_db::create_circuit_once(&self.pool, &circuit, Some(&message)).await
        {
            Ok(opened) => opened,
            Err(hydra_db::Error::Invalid(problems)) => {
                return Err(format!(
                    "{}; if this is another drawing, use its own drawing number as the name, \
                     otherwise ask the engineer",
                    problems.join("; ")
                ));
            }
            Err(e) => return Err(db_error(e)),
        };

        let status = CircuitStatus::compute(&circuit, &catalogue);
        let count = |s: ValveStatus| status.valves.values().filter(|(v, _)| *v == s).count();
        let (normal, amber, red, stand_in) = (
            count(ValveStatus::Normal),
            count(ValveStatus::PartNotListed),
            count(ValveStatus::Missing),
            count(ValveStatus::StandIn),
        );
        let warnings: Vec<String> = circuit
            .validate(&catalogue)
            .into_iter()
            .chain(status.issues)
            .filter(|i| i.severity() == Severity::Warning)
            .map(|i| i.to_string())
            .chain(wire_warnings(&circuit, &catalogue))
            // A cut is right for an L- or U-shaped block, but also hides a part left out by mistake.
            .chain(notched_parts(&circuit, &catalogue).into_iter().map(|id| {
                format!(
                    "{id} is outside the block and the outline is cut round it; check the drawing \
                     shows it outside the block, otherwise add it to in_block"
                )
            }))
            .collect();
        tracing::info!(
            name = circuit.name,
            components = circuit.components.len(),
            normal,
            part_not_listed = amber,
            missing = red,
            stand_in,
            warnings = warnings.len(),
            id = opened.id,
            "import_circuit saved"
        );
        let mut out = format!(
            "Circuit {:?} is saved in Hydra (id {}), marked as imported and unchecked.\n\
             Valves: {normal} in the catalogue, {amber} with a part not listed, {red} missing, \
             {stand_in} stand-ins; {} items, {} extra lines.\n",
            circuit.name,
            opened.id,
            circuit
                .components
                .iter()
                .filter(|c| matches!(c.kind, hydra_core::ComponentKind::Item(_)))
                .count(),
            circuit.bom_extra.len()
        );
        if warnings.is_empty() {
            out += "No warnings.\n";
        } else {
            out += "Warnings: check each against the drawing (an unconnected port usually means \
                    a missed connection):\n";
            for w in &warnings {
                out += &format!("- {w}\n");
            }
        }
        out += "Next: tell the engineer to open it in Hydra, check it against the drawing and \
                press Mark checked. Missing valves can be proposed from their datasheets.";
        Ok(out)
    }

    #[tool(
        description = "Status of a submitted proposal: pending, approved or rejected, who \
        reviewed it and their note (a rejection note says what to fix), and what was approved."
    )]
    pub async fn proposal_status(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<String, String> {
        let rec = hydra_db::get_proposal(&self.pool, &p.id)
            .await
            .map_err(db_error)?;
        let status = match rec.status {
            ProposalStatus::Pending => "pending, waiting for review",
            ProposalStatus::Approved => "approved",
            ProposalStatus::Rejected => "rejected",
        };
        let mut out = format!(
            "Proposal {} for {}: {status}.\nSubmitted by {} at {}.\n",
            rec.id,
            rec.proposal.target(),
            rec.created_by,
            rec.created_at
        );
        if let Some(by) = &rec.reviewed_by {
            out += &format!(
                "Reviewed by {by} at {}.\n",
                rec.reviewed_at.as_deref().unwrap_or("?")
            );
        }
        if let Some(note) = rec.review_note.as_deref().filter(|n| !n.trim().is_empty()) {
            out += &format!("Reviewer's note: {note}\n");
        }
        if let Some(approved) = &rec.approved {
            let edited = if *approved == rec.proposal {
                "as submitted"
            } else {
                "with the reviewer's edits"
            };
            out += &format!(
                "Approved {edited}:\n{}\n",
                serde_json::to_string_pretty(approved).expect("proposals serialize")
            );
        }
        Ok(out)
    }
}

#[tool_handler]
impl ServerHandler for HydraMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("hydra-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

impl HydraMcp {
    async fn load(&self) -> Result<LoadedCatalogue, String> {
        hydra_db::load_catalogue(&self.pool).await.map_err(db_error)
    }
}

fn db_error(e: hydra_db::Error) -> String {
    match e {
        hydra_db::Error::Invalid(_) => e.to_string(),
        e => format!("Hydra's database failed: {e}"),
    }
}

fn unreadable(why: &str) -> String {
    format!("could not read the datasheet: {why}. Ask the engineer to check the PDF.")
}

/// The `read_datasheet` reply for `pdf`, the datasheet of `name`, rendered by `renderer`
/// (a `hydra-mcp` program) in a child process.
pub async fn read_pdf(
    renderer: &Path,
    name: &str,
    pdf: Vec<u8>,
    p: &DatasheetParams,
) -> Result<CallToolResult, String> {
    let bytes_in = pdf.len();
    if bytes_in > MAX_PDF_BYTES {
        return Err(unreadable(&format!(
            "it is {} MB, more than the {} MB this server reads",
            bytes_in >> 20,
            MAX_PDF_BYTES >> 20
        )));
    }
    let first = p.first_page.unwrap_or(1);
    let count = p.max_pages.unwrap_or(DEFAULT_PAGES).clamp(1, MAX_PAGES);
    let last = first
        .saturating_add(count - 1)
        .min(p.last_page.unwrap_or(u32::MAX));
    let mut cmd = Command::new(renderer);
    cmd.args(["render-pdf", RENDER_PROTOCOL])
        .args([first, last, MAX_PX].map(|n| n.to_string()));
    let started = Instant::now();
    let rendered = run_renderer(cmd, pdf, RENDER_TIMEOUT).await;
    let ms = started.elapsed().as_millis();
    let rendered = rendered.map_err(|e| {
        tracing::warn!(key = name, bytes_in, ms, "datasheet not read: {e}");
        unreadable(&e)
    })?;
    let pages = rendered.pages;
    let reply = datasheet_reply(name, rendered, first, p.last_page)?;
    let bytes_out: usize = reply
        .content
        .iter()
        .map(|c| match c {
            ContentBlock::Text(t) => t.text.len(),
            ContentBlock::Image(i) => i.data.len(),
            _ => 0,
        })
        .sum();
    tracing::info!(key = name, pages, bytes_in, bytes_out, ms, "datasheet read");
    Ok(reply)
}

/// Runs `cmd`, a `hydra-mcp render-pdf` command, with `pdf` on its stdin, and reads its
/// [`Rendered`] output. The child is killed if it takes longer than `timeout`.
pub async fn run_renderer(
    mut cmd: Command,
    pdf: Vec<u8>,
    timeout: Duration,
) -> Result<Rendered, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    // CREATE_NO_WINDOW: no console window flashes up.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("the PDF reader did not start: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut out = Vec::new();
    let talk = async {
        // A child that exits early closes its stdin; its exit status tells why.
        let write = async {
            let _ = stdin.write_all(&pdf).await;
            drop(stdin);
        };
        let mut stdout = stdout.take(MAX_RENDER_OUTPUT as u64 + 1);
        let ((), read) = tokio::join!(write, stdout.read_to_end(&mut out));
        read.map_err(|e| format!("the PDF reader's output could not be read: {e}"))?;
        if out.len() > MAX_RENDER_OUTPUT {
            // It is still writing into a full pipe; waiting would only end in the timeout.
            let _ = child.kill().await;
            return Err("the PDF reader's output is too large".to_string());
        }
        child
            .wait()
            .await
            .map_err(|e| format!("the PDF reader failed: {e}"))
    };
    let status = match tokio::time::timeout(timeout, talk).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!(
                "reading it took too long, more than {} s",
                timeout.as_secs_f32()
            ));
        }
    };
    if !status.success() {
        return Err(format!("the PDF reader failed ({status})"));
    }
    serde_json::from_slice(&out).map_err(|e| format!("the PDF reader's output is garbled: {e}"))
}

/// The reply: a header and the text of the pages shown, then one image per page. Without
/// images, the text alone, as much as fits in [`MAX_CHARS`].
fn datasheet_reply(
    name: &str,
    out: Rendered,
    first: u32,
    last: Option<u32>,
) -> Result<CallToolResult, String> {
    let total = out.pages;
    if total == 0 {
        let why = out
            .text_error
            .into_iter()
            .chain(out.image_errors)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(unreadable(if why.is_empty() {
            "it has no pages"
        } else {
            &why
        }));
    }
    if first < 1 || first > total {
        return Err(format!(
            "first_page must be from 1 to {total}: the datasheet has {total} pages"
        ));
    }
    let mut head = format!("Datasheet of {name}, {total} pages.\n");
    for e in &out.image_errors {
        head += &format!("[Page image unavailable: {e}]\n");
    }
    let Some(shown) = out.images.last().map(|i| i.page) else {
        let pages = out.text.ok_or_else(|| {
            unreadable(out.text_error.as_deref().unwrap_or("the PDF reader failed"))
        })?;
        let text = select_pages(&pages, Some(first), last, MAX_CHARS)?;
        return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "{head}\n{text}"
        ))]));
    };
    let no_text = |why: &str| {
        let more = if shown < total {
            format!(
                " Call read_datasheet with first_page={} to read on.",
                shown + 1
            )
        } else {
            String::new()
        };
        format!("[Text unavailable: {why}. Read the page images.{more}]\n")
    };
    let text = match &out.text {
        Some(pages) if pages.iter().any(|t| !t.trim().is_empty()) => {
            select_pages(pages, Some(first), Some(shown), MAX_CHARS_WITH_IMAGES)
                .unwrap_or_else(|e| no_text(&e))
        }
        Some(_) => no_text("the PDF has no text, perhaps only scanned images"),
        None => no_text(out.text_error.as_deref().unwrap_or("the PDF reader failed")),
    };
    head += &format!("Its text, then an image of each page from {first} to {shown}.\n\n{text}");
    let mut content = vec![ContentBlock::text(head)];
    content.extend(
        out.images
            .into_iter()
            .map(|i| ContentBlock::image(i.png_base64, "image/png")),
    );
    Ok(CallToolResult::success(content))
}

fn latest_of<'a>(c: &'a Catalogue, key: &str) -> Result<&'a ValveType, String> {
    c.latest_valve_types().remove(key).ok_or_else(|| {
        format!("there is no valve type with key {key:?}; list_valve_types shows the keys")
    })
}

/// A proposal from the assistant's JSON, or what is wrong with it, with its path if known.
pub fn parse_proposal(value: serde_json::Value) -> Result<Proposal, String> {
    parse_json(json_object(value, "proposal")?, "proposal")
}

/// The JSON object the assistant sent as `what`. A JSON string holding the object is accepted
/// too: some assistants send objects that way.
fn json_object(value: serde_json::Value, what: &str) -> Result<serde_json::Value, String> {
    let value = match value {
        serde_json::Value::String(s) => {
            serde_json::from_str(&s).map_err(|e| format!("the {what} is not valid JSON: {e}"))?
        }
        v => v,
    };
    if !value.is_object() {
        return Err(format!(
            "the {what} must be a JSON object; symbol_format shows its shape"
        ));
    }
    Ok(value)
}

/// `value` as a `T`, or what is wrong with it, with its path if known.
fn parse_json<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    what: &str,
) -> Result<T, String> {
    serde_path_to_error::deserialize(value).map_err(|e| {
        let path = e.path().to_string();
        let inner = e.into_inner();
        if path == "." {
            format!("the {what} doesn't match the format in symbol_format: {inner}")
        } else {
            format!("the {what} doesn't match the format in symbol_format, at {path}: {inner}")
        }
    })
}

/// Pages `first..=last` (from 1) with page markers, as many as fit in `max_chars`, and a note
/// saying how to read the rest.
pub fn select_pages(
    pages: &[String],
    first: Option<u32>,
    last: Option<u32>,
    max_chars: usize,
) -> Result<String, String> {
    if pages.iter().all(|t| t.trim().is_empty()) {
        return Err(unreadable("it has no text, perhaps only scanned images"));
    }
    let total = pages.len();
    let first = first.unwrap_or(1) as usize;
    let last = last.map_or(total, |l| (l as usize).min(total));
    if first < 1 || first > total {
        return Err(format!(
            "first_page must be from 1 to {total}: the datasheet has {total} pages"
        ));
    }
    if last < first {
        return Err(format!("last_page ({last}) is before first_page ({first})"));
    }
    let mut out = String::new();
    let mut chars = 0;
    let mut shown = first - 1;
    for n in first..=last {
        let page = format!("--- page {n} ---\n{}\n", pages[n - 1].trim_end());
        let len = page.chars().count();
        if shown >= first && chars + len > max_chars {
            break;
        }
        // ponytail: one page longer than the cap is cut off, the rest of it unreachable;
        // page through by characters if real datasheets ever have such pages.
        if len > max_chars {
            out.extend(page.chars().take(max_chars));
            out += &format!(
                "\n[page {n} is cut off here: it is longer than {max_chars} characters]\n"
            );
        } else {
            out += &page;
        }
        chars += len;
        shown = n;
    }
    if shown < last {
        out += &format!(
            "\n[Showing pages {first}-{shown} of {total}, as much as fits in one call. Call \
             read_datasheet with first_page={} to read on.]\n",
            shown + 1
        );
    } else if last < total {
        out += &format!(
            "\n[Showing pages {first}-{last} of {total}. Call read_datasheet with first_page={} to \
             read on.]\n",
            last + 1
        );
    } else if first > 1 {
        out += &format!("\n[Showing pages {first}-{last} of {total}.]\n");
    }
    Ok(out)
}

/// The symbol and proposal guide, then every existing symbol definition as `key@version` and
/// its JSON.
pub fn symbol_guide(c: &Catalogue) -> String {
    let mut out = GUIDE.to_string();
    for s in &c.symbol_defs {
        out += &format!(
            "{}  {}\n",
            s.reference(),
            serde_json::to_string(&s.definition).expect("symbol definitions serialize")
        );
    }
    if c.symbol_defs.is_empty() {
        out += "none yet\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hydra_core::CatalogueRef;

    fn fixture() -> Catalogue {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn good() -> serde_json::Value {
        serde_json::json!({
            "valve_type": {"key": "vt-x", "version": 0, "manufacturer": "demo",
                "series": "S", "code": "C", "symbol": "sym-check@1"},
            "parts": [{"part_number": "X-1"}],
            "notes": "page 1"
        })
    }

    #[test]
    fn a_proposal_parses_from_an_object_or_a_json_string() {
        let p = parse_proposal(good()).unwrap();
        assert_eq!(p.target(), CatalogueRef::new("vt-x", 1));
        assert_eq!(parse_proposal(good().to_string().into()).unwrap(), p);
    }

    #[test]
    fn parse_errors_say_where() {
        let mut v = good();
        v["valve_type"]["symbol"] = "sym-check".into();
        let e = parse_proposal(v).unwrap_err();
        assert!(e.contains("at valve_type.symbol"), "{e}");
        assert!(e.contains("key@version"), "{e}");

        let mut v = good();
        v["valve_type"].as_object_mut().unwrap().remove("series");
        let e = parse_proposal(v).unwrap_err();
        assert!(e.contains("at valve_type") && e.contains("`series`"), "{e}");

        let mut v = good();
        v["parts"][0]["part_number"] = 7.into();
        let e = parse_proposal(v).unwrap_err();
        assert!(e.contains("at parts[0].part_number"), "{e}");

        let e = parse_proposal(serde_json::json!([1])).unwrap_err();
        assert!(e.starts_with("the proposal must be a JSON object"), "{e}");
        let e = parse_proposal(serde_json::json!({"notes": "x"})).unwrap_err();
        assert_eq!(
            e,
            "the proposal doesn't match the format in symbol_format: missing field `valve_type`"
        );
        let e = parse_proposal("{oops".into()).unwrap_err();
        assert!(e.starts_with("the proposal is not valid JSON"), "{e}");
    }

    fn rendered(text: Option<Vec<String>>, images: &[u32]) -> Rendered {
        Rendered {
            pages: 3,
            text,
            images: images
                .iter()
                .map(|&page| PageImage {
                    page,
                    png_base64: format!("png{page}"),
                })
                .collect(),
            ..Rendered::default()
        }
    }

    fn texts(r: &CallToolResult) -> Vec<&str> {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect()
    }

    fn images(r: &CallToolResult) -> Vec<&str> {
        r.content
            .iter()
            .filter_map(|c| c.as_image().map(|i| i.data.as_str()))
            .collect()
    }

    #[test]
    fn the_reply_has_the_text_of_the_pages_shown_then_their_images() {
        let text = Some(pages(3, 5));
        let r = datasheet_reply("vt@1", rendered(text.clone(), &[1, 2]), 1, None).unwrap();
        assert_eq!(images(&r), ["png1", "png2"]);
        let [t] = texts(&r)[..] else { panic!("{r:?}") };
        assert!(t.starts_with("Datasheet of vt@1, 3 pages.\n"), "{t}");
        assert!(
            t.contains("--- page 2 ---") && !t.contains("--- page 3 ---"),
            "{t}"
        );
        assert!(t.contains("first_page=3"), "{t}");
        // The text beside images is capped, with a note on where to read on.
        let long = Some(pages(3, MAX_CHARS_WITH_IMAGES));
        let r = datasheet_reply("vt@1", rendered(long, &[1, 2]), 1, None).unwrap();
        let t = texts(&r)[0];
        assert!(
            t.contains("page 1 is cut off") && t.contains("first_page=2"),
            "{t}"
        );
        assert!(t.len() < MAX_CHARS_WITH_IMAGES + 500, "{}", t.len());

        // Without text, or with no text in it, the images still come with a note.
        let mut out = rendered(None, &[3]);
        out.text_error = Some("bad font".into());
        let r = datasheet_reply("vt@1", out, 3, None).unwrap();
        assert!(
            texts(&r)[0].contains("[Text unavailable: bad font."),
            "{r:?}"
        );
        assert_eq!(images(&r), ["png3"]);
        let blank = Some(vec![String::new(); 3]);
        let r = datasheet_reply("vt@1", rendered(blank, &[1]), 1, None).unwrap();
        assert!(texts(&r)[0].contains("[Text unavailable: the PDF has no text"));
        assert_eq!(images(&r), ["png1"]);
    }

    #[test]
    fn without_images_the_reply_is_the_text_alone() {
        let mut out = rendered(Some(pages(3, 5)), &[]);
        out.image_errors = vec!["page 1: boom".into()];
        let r = datasheet_reply("vt@1", out, 1, None).unwrap();
        assert!(images(&r).is_empty());
        let t = texts(&r)[0];
        assert!(t.contains("[Page image unavailable: page 1: boom]"), "{t}");
        assert!(t.contains("--- page 3 ---"), "{t}");

        let e = datasheet_reply("vt@1", rendered(Some(pages(3, 5)), &[]), 4, None).unwrap_err();
        assert!(e.contains("has 3 pages"), "{e}");
        let mut out = rendered(None, &[]);
        out.text_error = Some("bad xref".into());
        let e = datasheet_reply("vt@1", out, 1, None).unwrap_err();
        assert!(
            e.starts_with("could not read the datasheet: bad xref."),
            "{e}"
        );
        let out = Rendered {
            text_error: Some("bad header".into()),
            image_errors: vec!["can't open".into()],
            ..Rendered::default()
        };
        let e = datasheet_reply("vt@1", out, 1, None).unwrap_err();
        assert!(e.contains("bad header; can't open"), "{e}");
    }

    fn pages(n: usize, len: usize) -> Vec<String> {
        (1..=n).map(|i| format!("{i}").repeat(len)).collect()
    }

    #[test]
    fn pages_are_marked_and_selected() {
        let p = pages(3, 5);
        assert_eq!(
            select_pages(&p, None, None, 1000).unwrap(),
            "--- page 1 ---\n11111\n--- page 2 ---\n22222\n--- page 3 ---\n33333\n"
        );
        assert_eq!(
            select_pages(&p, Some(2), Some(2), 1000).unwrap(),
            "--- page 2 ---\n22222\n\n[Showing pages 2-2 of 3. Call read_datasheet with \
             first_page=3 to read on.]\n"
        );
        assert_eq!(
            select_pages(&p, Some(2), None, 1000).unwrap(),
            "--- page 2 ---\n22222\n--- page 3 ---\n33333\n\n[Showing pages 2-3 of 3.]\n"
        );
        // A last page past the end means up to the end.
        assert!(
            select_pages(&p, Some(3), Some(9), 1000)
                .unwrap()
                .contains("pages 3-3 of 3")
        );
        assert!(
            select_pages(&p, Some(0), None, 1000)
                .unwrap_err()
                .contains("from 1 to 3")
        );
        assert!(
            select_pages(&p, Some(4), None, 1000)
                .unwrap_err()
                .contains("from 1 to 3")
        );
        assert!(
            select_pages(&p, Some(3), Some(2), 1000)
                .unwrap_err()
                .contains("before")
        );
        assert!(
            select_pages(&p, None, Some(2), 1000)
                .unwrap()
                .ends_with("--- page 2 ---\n22222\n\n[Showing pages 1-2 of 3. Call read_datasheet with first_page=3 to read on.]\n")
        );
        // No text at all, as in a scanned PDF, is an error; a blank page among others is not.
        let blank = vec![" \n".to_string(), String::new()];
        assert!(
            select_pages(&blank, None, None, 1000)
                .unwrap_err()
                .contains("no text, perhaps only scanned images")
        );
        assert!(select_pages(&[], None, None, 1000).is_err());
        let one_blank = vec!["x".to_string(), String::new()];
        assert!(
            select_pages(&one_blank, None, None, 1000)
                .unwrap()
                .ends_with("--- page 2 ---\n\n")
        );
    }

    #[test]
    fn long_output_is_capped_with_a_note_on_the_rest() {
        // Each page is 15 + 100 + 1 = 116 characters with its marker.
        let p = pages(10, 100);
        let out = select_pages(&p, None, None, 300).unwrap();
        assert!(
            out.contains("--- page 2 ---") && !out.contains("--- page 3 ---"),
            "{out}"
        );
        assert!(
            out.contains("pages 1-2 of 10") && out.contains("first_page=3"),
            "{out}"
        );
        let out = select_pages(&p, Some(3), None, 300).unwrap();
        assert!(
            out.starts_with("--- page 3 ---") && out.contains("first_page=5"),
            "{out}"
        );
        // Exactly at the cap still fits.
        assert!(
            !select_pages(&p, Some(1), Some(2), 232)
                .unwrap()
                .contains("as much as fits")
        );
        // One page over the cap is cut, and still counts as read.
        let out = select_pages(&p, Some(1), Some(2), 50).unwrap();
        assert!(
            out.contains("page 1 is cut off") && out.contains("first_page=2"),
            "{out}"
        );
        assert!(out.chars().filter(|c| *c == '1').count() < 50, "{out}");
    }

    #[test]
    fn the_guide_lists_every_symbol_definition() {
        let c = fixture();
        let guide = symbol_guide(&c);
        assert!(guide.starts_with(GUIDE));
        assert!(!c.symbol_defs.is_empty());
        for s in &c.symbol_defs {
            let line = format!(
                "{}  {}\n",
                s.reference(),
                serde_json::to_string(&s.definition).unwrap()
            );
            assert!(guide.contains(&line), "missing {line}");
        }
        assert!(symbol_guide(&Catalogue::default()).ends_with("none yet\n"));
    }

    #[test]
    fn the_guide_example_proposal_and_symbols_parse() {
        // The guide's example, with the elided parts filled in, is a valid proposal shape.
        let mut example: serde_json::Value = {
            let start = GUIDE.find("{\n  \"valve_type\"").unwrap();
            let end = start + GUIDE[start..].find("\n}\n").unwrap() + 2;
            let text = GUIDE[start..end]
                .replace("{...}", r#"{"kind": "check"}"#)
                .replace("\"...\"", "\"x\"");
            serde_json::from_str(&text).unwrap()
        };
        example["new_symbol"]["key"] = "sym-x".into();
        parse_proposal(example).unwrap();
        for kind in [
            r#"{"kind": "check", "spring": true}"#,
            r#"{"kind": "orifice", "adjustable": false}"#,
            r#"{"kind": "relief", "adjustable": true}"#,
        ] {
            assert!(GUIDE.contains(kind));
            serde_json::from_str::<hydra_core::SymbolDef>(kind).unwrap();
        }
    }
}
