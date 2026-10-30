//! MCP tools that let an AI assistant fill in draft valve types from their datasheets.
//!
//! The assistant reads the catalogue and the attached PDF, and submits a [`Proposal`]. Every
//! proposal is checked with [`Proposal::problems`]; valid ones wait in Hydra's review queue, and
//! only an engineer's approval writes catalogue rows.

use std::collections::BTreeMap;

use hydra_core::{Catalogue, Proposal, ValveType};
use hydra_db::{LoadedCatalogue, PgPool, ProposalStatus};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

/// Sent at initialize: the workflow, in a few lines.
pub const INSTRUCTIONS: &str = "\
Hydra is a hydraulic CAD app. With these tools you fill in draft valve types from their \
manufacturer datasheets; an engineer reviews every proposal in Hydra before anything enters \
the catalogue.
1. list_valve_types: find the draft the engineer means (symbol: none, datasheet: yes).
2. read_datasheet: read its PDF.
3. symbol_format: read the symbol and proposal format and the existing symbol definitions.
4. Reuse an existing symbol definition that shows the same valve, or define a new one.
5. submit_proposal: it is checked at once; fix every problem it reports and submit again.
6. Tell the engineer the proposal id and to review it in Hydra's Proposals tab.
Never invent part numbers or values that aren't in the datasheet; put doubts in the notes.";

/// The `symbol_format` guide, before the list of existing symbol definitions.
const GUIDE: &str = r#"# Symbol definitions

A symbol definition says what an ISO 1219 valve symbol shows; Hydra draws it. It is JSON tagged by "kind":

{"kind": "check", "spring": true}         non-return valve, free flow A -> B; spring-loaded if spring
{"kind": "orifice", "adjustable": false}  restriction between A and B; a throttle if adjustable
{"kind": "relief", "adjustable": true}    pressure relief valve from P to T
{"kind": "directional_valve", ...}        spool valve, see below

check and orifice always have ports A and B, relief always P and T; they take no other fields.

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
    "mounting": "ISO 4401-05", "params": {"max_pressure_bar": 315, "max_flow_l_min": 120}
  },
  "new_symbol": {"key": "sym-...", "version": 1, "definition": {...}},
  "parts": [{"part_number": "...", "options": {"voltage": "24 V DC", "connector": "..."}}],
  "notes": "..."
}

Rules:
- valve_type.version is the base: the latest version of the key you update (list_valve_types, get_valve_type), or 0 for a new key. Approval saves key@(version + 1).
- manufacturer is a manufacturer key from list_valve_types. series and code must not be empty. Leave mounting out if the valve has no subplate pattern. params is a free JSON object of datasheet values, with units in the names.
- symbol is "key@version". Reuse an existing definition below when it shows exactly this valve. Otherwise add new_symbol (leave it out when reusing): its version is the next of its key (1 for a new key), and valve_type.symbol must be new_symbol's key@version.
- parts are the orderable part numbers of the new version, with their ordering-code options. Parts of the base version are kept, unless you list the same part number again to replace its options. Don't list a part twice.
- notes cite the datasheet page of every value (e.g. "pressure and flow: page 1; part numbers: page 2") and say anything the reviewer should check.
- Never invent part numbers or values that aren't in the datasheet; put doubts in notes.

# Existing symbol definitions
"#;

/// Datasheet text returned in one call, in characters.
const MAX_CHARS: usize = 60_000;
/// Largest datasheet read; manufacturer datasheets are a few MB.
const MAX_PDF_BYTES: usize = 64 << 20;
const PDF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
    /// Last page to read. Default: the last page.
    #[serde(default)]
    pub last_page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposalParams {
    /// The proposal: a JSON object with valve_type, optional new_symbol, parts and notes, as
    /// symbol_format describes.
    pub proposal: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdParams {
    /// Proposal id, as submit_proposal returned it.
    pub id: String,
}

/// The MCP server: one database pool, the engineer's own role.
#[derive(Clone)]
pub struct HydraMcp {
    pool: PgPool,
}

#[tool_router]
impl HydraMcp {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        let rows: Vec<_> = latest_versions(c)
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
        out += "\nValve types (key@latest version  manufacturer  series code  mounting  symbol  datasheet):\n";
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
            out += &format!(
                "  {r}  {}  {} {}  {}  symbol: {symbol}  datasheet: {}",
                v.manufacturer,
                v.series,
                v.code,
                v.mounting.as_deref().unwrap_or("-"),
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
        description = "Text of the PDF datasheet attached to a valve type's latest version, \
        with --- page N --- markers. Long datasheets come in parts: pass first_page and \
        last_page to read the rest."
    )]
    pub async fn read_datasheet(
        &self,
        Parameters(p): Parameters<DatasheetParams>,
    ) -> Result<String, String> {
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
        if bytes.len() > MAX_PDF_BYTES {
            return Err(unreadable(&format!(
                "it is {} MB, more than the {} MB this server reads",
                bytes.len() >> 20,
                MAX_PDF_BYTES >> 20
            )));
        }
        // pdf-extract can panic on unusual PDFs; on the blocking pool a panic is just an Err.
        // ponytail: a reader stuck in a loop keeps its thread after the timeout; run extraction
        // in a child process if hostile PDFs ever matter.
        let pages = tokio::time::timeout(
            PDF_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                pdf_extract::extract_text_from_mem_by_pages(&bytes)
            }),
        )
        .await;
        let pages = match pages {
            Ok(Ok(Ok(pages))) => pages,
            Ok(Ok(Err(e))) => return Err(unreadable(&e.to_string())),
            Ok(Err(_)) => return Err(unreadable("the PDF reader failed")),
            Err(_) => return Err(unreadable("reading it took too long")),
        };
        let text = select_pages(&pages, p.first_page, p.last_page, MAX_CHARS)?;
        Ok(format!(
            "Datasheet of {r}, {} pages.\n\n{text}",
            pages.len()
        ))
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
            Err(e) => Err(db_error(e)),
        }
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
    format!(
        "could not read the text of the datasheet: {why}. Ask the engineer to check the PDF, or \
         to attach one with selectable text."
    )
}

/// The latest version of every valve type key, by key.
fn latest_versions(c: &Catalogue) -> BTreeMap<&str, &ValveType> {
    let mut latest: BTreeMap<&str, &ValveType> = BTreeMap::new();
    for v in &c.valve_types {
        let e = latest.entry(&v.key).or_insert(v);
        if v.version > e.version {
            *e = v;
        }
    }
    latest
}

fn latest_of<'a>(c: &'a Catalogue, key: &str) -> Result<&'a ValveType, String> {
    latest_versions(c).remove(key).ok_or_else(|| {
        format!("there is no valve type with key {key:?}; list_valve_types shows the keys")
    })
}

/// A proposal from the assistant's JSON, or what is wrong with it, with its path if known.
/// A JSON string holding the object is accepted too: some assistants send objects that way.
pub fn parse_proposal(value: serde_json::Value) -> Result<Proposal, String> {
    let value = match value {
        serde_json::Value::String(s) => {
            serde_json::from_str(&s).map_err(|e| format!("the proposal is not valid JSON: {e}"))?
        }
        v => v,
    };
    if !value.is_object() {
        return Err("the proposal must be a JSON object; symbol_format shows its shape".into());
    }
    serde_path_to_error::deserialize(value).map_err(|e| {
        let path = e.path().to_string();
        let inner = e.into_inner();
        if path == "." {
            format!("the proposal doesn't match the format in symbol_format: {inner}")
        } else {
            format!("the proposal doesn't match the format in symbol_format, at {path}: {inner}")
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
    } else if first > 1 || last < total {
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

    #[test]
    fn the_demo_datasheet_reads_back() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/datasheets/demo-10-datasheet.pdf"
        );
        let pages =
            pdf_extract::extract_text_from_mem_by_pages(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(pages.len(), 2);
        for text in [
            "4/3 directional valve",
            "ISO 4401-05",
            "315 bar",
            "120 l/min",
        ] {
            assert!(pages[0].contains(text), "{text} not in {:?}", pages[0]);
        }
        for text in ["DEMO-10-4-3-C-24DC", "DEMO-10-4-3-C-230AC", "EN 175301-803"] {
            assert!(pages[1].contains(text), "{text} not in {:?}", pages[1]);
        }
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
            "--- page 2 ---\n22222\n\n[Showing pages 2-2 of 3.]\n"
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
                .ends_with("--- page 2 ---\n22222\n\n[Showing pages 1-2 of 3.]\n")
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
                .contains("first_page")
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
