//! The tools against a real database: tool handlers called directly, and one stdio session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use hydra_core::{Catalogue, CatalogueRef, Circuit, Proposal, ValveType};
use hydra_db::{
    ProposalStatus, approve_proposal, attach_datasheet, import_catalogue, list_circuits,
    list_proposals, list_revisions, open_circuit, reject_proposal, save_valve_type,
};
use hydra_mcp::{
    CircuitParams, DatasheetParams, HydraMcp, IdParams, KeyParams, ProposalParams, SearchParams,
};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use sqlx::PgPool;

const PDF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/datasheets/demo-10-datasheet.pdf"
);

/// The fixture catalogue plus a draft vt-demo-10@1 with the demo datasheet attached.
async fn setup(pool: &PgPool) -> HydraMcp {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/catalogue.json");
    let fixture: Catalogue = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    import_catalogue(pool, &fixture).await.unwrap();
    let draft = ValveType {
        key: "vt-demo-10".into(),
        version: 0,
        manufacturer: "demo".into(),
        series: "DEMO-10".into(),
        code: "4-3-C".into(),
        symbol: None,
        mounting: Some("ISO 4401-05".into()),
        params: Value::Null,
    };
    let r = save_valve_type(pool, &draft).await.unwrap();
    assert_eq!(r, CatalogueRef::new("vt-demo-10", 1));
    attach_datasheet(pool, &r, &std::fs::read(PDF).unwrap())
        .await
        .unwrap();
    HydraMcp::new(pool.clone(), env!("CARGO_BIN_EXE_hydra-mcp").into())
}

/// What an assistant would submit for the demo draft after reading its datasheet.
fn good_proposal() -> Value {
    json!({
        "valve_type": {
            "key": "vt-demo-10", "version": 1, "manufacturer": "demo",
            "series": "DEMO-10", "code": "4-3-C", "symbol": "sym-4-3-closed-centre@1",
            "mounting": "ISO 4401-05", "params": {"max_pressure_bar": 315, "max_flow_l_min": 120}
        },
        "parts": [
            {"part_number": "DEMO-10-4-3-C-24DC", "options": {"voltage": "24 V DC", "connector": "EN 175301-803"}},
            {"part_number": "DEMO-10-4-3-C-230AC", "options": {"voltage": "230 V AC", "connector": "EN 175301-803"}}
        ],
        "notes": "page 1: symbol, pressure, flow, mounting; page 2: ordering codes"
    })
}

fn key(k: &str) -> Parameters<KeyParams> {
    Parameters(KeyParams { key: k.into() })
}

fn datasheet(k: &str, first: Option<u32>, last: Option<u32>) -> Parameters<DatasheetParams> {
    Parameters(DatasheetParams {
        key: k.into(),
        first_page: first,
        last_page: last,
        max_pages: None,
    })
}

/// The text of a read_datasheet reply; the page images follow it.
fn text_of(r: rmcp::model::CallToolResult) -> String {
    r.content[0].as_text().unwrap().text.clone()
}

fn submit(proposal: Value) -> Parameters<ProposalParams> {
    Parameters(ProposalParams { proposal })
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn the_catalogue_and_the_datasheet_can_be_read(pool: PgPool) {
    let mcp = setup(&pool).await;

    let list = mcp
        .list_valve_types(Parameters(SearchParams { search: None }))
        .await
        .unwrap();
    assert!(list.contains("  demo  Demo (fixture data"), "{list}");
    let draft_line = list.lines().find(|l| l.contains("vt-demo-10@1")).unwrap();
    assert!(
        draft_line.contains("symbol: none (draft)  datasheet: yes  <- draft with a datasheet"),
        "{draft_line}"
    );
    assert!(list.contains("vt-demo-4-2@1  demo  DEMO-06 4-2-S  ISO 4401-03  directional  symbol: sym-4-2-spring-return@1  datasheet: no\n"), "{list}");
    let found = mcp
        .list_valve_types(Parameters(SearchParams {
            search: Some("4401-05".into()),
        }))
        .await
        .unwrap();
    assert!(
        found.contains("vt-demo-10@1") && !found.contains("vt-demo-4-2"),
        "{found}"
    );

    let vt: Value =
        serde_json::from_str(&mcp.get_valve_type(key("vt-demo-10")).await.unwrap()).unwrap();
    assert_eq!(vt["latest"]["version"], 1);
    assert_eq!(vt["symbol"], Value::Null);
    assert_eq!(vt["datasheet_attached"], true);
    assert_eq!(vt["versions"].as_array().unwrap().len(), 1);
    let vt: Value =
        serde_json::from_str(&mcp.get_valve_type(key("vt-demo-4-2")).await.unwrap()).unwrap();
    assert_eq!(vt["symbol"]["ref"], "sym-4-2-spring-return@1");
    assert_eq!(vt["symbol"]["definition"]["kind"], "directional_valve");
    assert_eq!(vt["parts"].as_array().unwrap().len(), 2);
    assert_eq!(vt["datasheet_attached"], false);
    assert!(
        mcp.get_valve_type(key("vt-nope"))
            .await
            .unwrap_err()
            .contains("no valve type")
    );

    let text = text_of(
        mcp.read_datasheet(datasheet("vt-demo-10", None, None))
            .await
            .unwrap(),
    );
    assert!(
        text.starts_with("Datasheet of vt-demo-10@1, 2 pages."),
        "{text}"
    );
    assert!(
        text.contains("--- page 1 ---") && text.contains("--- page 2 ---"),
        "{text}"
    );
    assert!(
        text.contains("315 bar") && text.contains("DEMO-10-4-3-C-230AC"),
        "{text}"
    );
    let page2 = text_of(
        mcp.read_datasheet(datasheet("vt-demo-10", Some(2), None))
            .await
            .unwrap(),
    );
    assert!(
        !page2.contains("--- page 1 ---") && page2.contains("pages 2-2 of 2"),
        "{page2}"
    );
    let e = mcp
        .read_datasheet(datasheet("vt-demo-4-2", None, None))
        .await
        .unwrap_err();
    assert!(e.contains("has no datasheet attached"), "{e}");
    let e = mcp
        .read_datasheet(datasheet("vt-demo-10", Some(3), None))
        .await
        .unwrap_err();
    assert!(e.contains("has 2 pages"), "{e}");

    let guide = mcp.symbol_format().await.unwrap();
    assert!(
        guide.contains("sym-4-3-closed-centre@1  {\"kind\":\"directional_valve\""),
        "{guide}"
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn proposals_are_checked_then_queued(pool: PgPool) {
    let mcp = setup(&pool).await;

    let mut bad = good_proposal();
    bad["valve_type"]["version"] = 0.into();
    bad["valve_type"]["manufacturer"] = "acme".into();
    let e = mcp.submit_proposal(submit(bad)).await.unwrap_err();
    assert_eq!(
        e,
        "The proposal was not accepted. Fix these problems and submit again:\n\
         - valve type vt-demo-10 already exists at version 1; use version 1 to update it\n\
         - params.function is missing; say what the valve does, one of: directional, check, \
         pilot_operated_check, relief, counterbalance, flow_control, pressure_reducing, sequence, \
         other\n\
         - unknown manufacturer \"acme\"; use one of these keys: argo-hytos, demo"
    );
    let mut bad = good_proposal();
    bad["valve_type"]["manufacturer"] = "acme".into();
    bad["valve_type"]["symbol"] = "sym-nope@1".into();
    let e = mcp.submit_proposal(submit(bad)).await.unwrap_err();
    assert!(
        e.contains("\n- unknown manufacturer \"acme\"")
            && e.contains("\n- unknown symbol sym-nope@1;"),
        "{e}"
    );
    let mut bad = good_proposal();
    bad["valve_type"]["symbol"] = "sym-4-3-closed-centre".into();
    let e = mcp.submit_proposal(submit(bad)).await.unwrap_err();
    assert!(e.contains("at valve_type.symbol"), "{e}");
    assert!(list_proposals(&pool, false).await.unwrap().is_empty());

    let ok = mcp.submit_proposal(submit(good_proposal())).await.unwrap();
    let id = ok
        .strip_prefix("Proposal ")
        .and_then(|s| {
            s.strip_suffix(" for vt-demo-10@2 is waiting for review in Hydra's Proposals tab.")
        })
        .unwrap_or_else(|| panic!("{ok}"));
    let queued = list_proposals(&pool, true).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, id);
    assert_eq!(queued[0].status, ProposalStatus::Pending);
    assert_eq!(queued[0].proposal.parts.len(), 2);

    let e = mcp
        .submit_proposal(submit(good_proposal()))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        format!(
            "The proposal was not accepted: proposal {id} for vt-demo-10@1 is still waiting for \
             review, and only one of them could be approved. Don't submit it again. Tell the \
             engineer to approve or reject proposal {id} in Hydra's Proposals tab; if they \
             reject it, read their note with proposal_status and submit a corrected proposal."
        )
    );
    assert_eq!(list_proposals(&pool, true).await.unwrap().len(), 1);

    let status = mcp
        .proposal_status(Parameters(IdParams { id: id.into() }))
        .await
        .unwrap();
    assert!(
        status.starts_with(&format!(
            "Proposal {id} for vt-demo-10@2: pending, waiting for review.\nSubmitted by "
        )),
        "{status}"
    );
    let e = mcp
        .proposal_status(Parameters(IdParams { id: "nope".into() }))
        .await
        .unwrap_err();
    assert_eq!(e, "unknown proposal");
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn the_status_tells_the_review_outcome(pool: PgPool) {
    let mcp = setup(&pool).await;
    let status = |id: String| {
        let mcp = mcp.clone();
        async move {
            mcp.proposal_status(Parameters(IdParams { id }))
                .await
                .unwrap()
        }
    };
    let id_of = |ok: String| ok.split(' ').nth(1).unwrap().to_string();

    let rejected = id_of(mcp.submit_proposal(submit(good_proposal())).await.unwrap());
    reject_proposal(
        &pool,
        &rejected,
        "flow is 120 l/min only up to 250 bar, see page 1",
    )
    .await
    .unwrap();
    let s = status(rejected).await;
    assert!(
        s.contains(": rejected.\n") && s.contains("\nReviewed by "),
        "{s}"
    );
    assert!(
        s.contains("Reviewer's note: flow is 120 l/min only up to 250 bar, see page 1\n"),
        "{s}"
    );
    assert!(!s.contains("Approved"), "{s}");

    let approved = id_of(mcp.submit_proposal(submit(good_proposal())).await.unwrap());
    let mut edited: Proposal = serde_json::from_value(good_proposal()).unwrap();
    edited.parts.pop();
    approve_proposal(&pool, &approved, &edited).await.unwrap();
    let s = status(approved).await;
    assert!(
        s.contains(": approved.\n") && s.contains("\nReviewed by "),
        "{s}"
    );
    let json = s
        .split_once("Approved with the reviewer's edits:\n")
        .unwrap()
        .1;
    assert_eq!(serde_json::from_str::<Proposal>(json).unwrap(), edited);

    // The tools now show the approved version as the latest, with its symbol and parts.
    let vt: Value =
        serde_json::from_str(&mcp.get_valve_type(key("vt-demo-10")).await.unwrap()).unwrap();
    assert_eq!(vt["latest"]["version"], 2);
    assert_eq!(vt["versions"].as_array().unwrap().len(), 2);
    assert_eq!(vt["symbol"]["ref"], "sym-4-3-closed-centre@1");
    assert_eq!(vt["parts"].as_array().unwrap().len(), 1);
    assert_eq!(vt["datasheet_attached"], true);
    let list = mcp
        .list_valve_types(Parameters(SearchParams { search: None }))
        .await
        .unwrap();
    assert!(
        list.contains("vt-demo-10@2  demo  DEMO-10 4-3-C  ISO 4401-05  directional  symbol: sym-4-3-closed-centre@1  datasheet: yes\n"),
        "{list}"
    );
    assert!(!list.contains("vt-demo-10@1"), "{list}");
}

/// Reads JSON-RPC lines until the response with `id`.
fn response(out: &mut impl BufRead, id: u64) -> Value {
    loop {
        let mut line = String::new();
        assert!(
            out.read_line(&mut line).unwrap() > 0,
            "server closed stdout"
        );
        // stdout carries the protocol and nothing else.
        let msg: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(msg["jsonrpc"], "2.0", "{line}");
        if msg["id"] == id {
            return msg;
        }
    }
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn a_stdio_session_works(pool: PgPool) {
    setup(&pool).await;
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    // ponytail: assumes DATABASE_URL has no ?query part, as in the README.
    let url = std::env::var("DATABASE_URL").unwrap();
    let url = format!("{}/{db}", url.rsplit_once('/').unwrap().0);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hydra-mcp"))
        .env("HYDRA_DATABASE_URL", url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let logs = std::thread::spawn(move || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut stderr, &mut s).unwrap();
        s
    });
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: Value| writeln!(stdin, "{msg}").unwrap();

    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}}),
    );
    let init = response(&mut out, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "hydra-mcp", "{init}");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("read_datasheet")
    );
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let list = response(&mut out, 2);
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "get_valve_type",
            "import_circuit",
            "list_valve_types",
            "proposal_status",
            "read_datasheet",
            "submit_proposal",
            "symbol_format"
        ]
    );

    send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
        "name": "read_datasheet", "arguments": {"key": "vt-demo-10"}}}),
    );
    let read = response(&mut out, 3);
    assert_ne!(read["result"]["isError"], true, "{read}");
    let content = &read["result"]["content"];
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("--- page 2 ---") && text.contains("DEMO-10-4-3-C-24DC"),
        "{text}"
    );
    for page in [1, 2] {
        assert_eq!(content[page]["type"], "image", "{content}");
        assert_eq!(content[page]["mimeType"], "image/png", "{content}");
    }

    send(
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
        "name": "submit_proposal", "arguments": {"proposal": {"notes": "empty"}}}}),
    );
    let bad = response(&mut out, 4);
    assert_eq!(bad["result"]["isError"], true, "{bad}");
    assert!(
        bad["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("missing field `valve_type`")
    );

    send(
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
        "name": "import_circuit", "arguments": {"circuit": demo_circuit("demo-import-motors")}}}),
    );
    let imported = response(&mut out, 5);
    assert_ne!(imported["result"]["isError"], true, "{imported}");

    drop(stdin);
    assert!(child.wait().unwrap().success());
    let logs = logs.join().unwrap();
    assert!(
        logs.contains("datasheet read") && logs.contains("key=\"vt-demo-10@1\""),
        "{logs}"
    );
    assert!(
        logs.contains("import_circuit saved") && logs.contains("missing=2"),
        "{logs}"
    );
}

fn demo_circuit(name: &str) -> Value {
    let path = format!(
        "{}/../../fixtures/circuits/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn import(circuit: Value) -> Parameters<CircuitParams> {
    Parameters(CircuitParams { circuit })
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn imported_circuits_are_saved_as_unchecked_drafts(pool: PgPool) {
    let mcp = setup(&pool).await;
    let me: String = sqlx::query_scalar("SELECT session_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    for name in ["demo-import-manifold", "demo-import-motors"] {
        let json = demo_circuit(name);
        let reply = mcp.import_circuit(import(json.clone())).await.unwrap();
        assert!(
            reply.contains("marked as imported and unchecked"),
            "{reply}"
        );
        let summary = list_circuits(&pool)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == json["name"])
            .unwrap();
        assert_eq!(summary.owner, me);
        let opened = open_circuit(&pool, &summary.id).await.unwrap();
        assert_eq!(opened.revision, 1);
        let expected = Circuit::from_json(&json.to_string()).unwrap();
        assert_eq!(opened.document, expected, "{name}");
        let revisions = list_revisions(&pool, &summary.id).await.unwrap();
        assert_eq!(revisions[0].created_by, me);
        assert!(
            revisions[0]
                .message
                .as_deref()
                .unwrap()
                .starts_with("Imported by the AI assistant from drawing"),
            "{revisions:?}"
        );
    }
    let reply = mcp
        .import_circuit(import(demo_circuit("demo-import-manifold")))
        .await;
    assert!(reply.unwrap_err().contains("already exists"));
    assert_eq!(list_circuits(&pool).await.unwrap().len(), 2);

    // A retry racing the first attempt still makes one circuit.
    let mut json = demo_circuit("demo-import-motors");
    json["name"] = "DEMO-RACE".into();
    let (a, b) = tokio::join!(
        mcp.import_circuit(import(json.clone())),
        mcp.import_circuit(import(json))
    );
    assert!(a.is_ok() != b.is_ok(), "{a:?} / {b:?}");
    assert_eq!(list_circuits(&pool).await.unwrap().len(), 3);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn problems_are_listed_and_nothing_is_saved(pool: PgPool) {
    let mcp = setup(&pool).await;
    let mut json = demo_circuit("demo-import-manifold");
    json["name"] = "DEMO-BROKEN".into();
    // A missing valve that doesn't say what it is, and a relief drawn as an item.
    let components = json["components"].as_array_mut().unwrap();
    for c in components.iter_mut() {
        if c["id"] == "5.2" {
            c["requested"].as_object_mut().unwrap().remove("function");
        }
        if c["id"] == "X1" {
            c["symbol"] = json!({"kind": "relief"});
        }
    }
    let err = mcp.import_circuit(import(json)).await.unwrap_err();
    assert!(err.contains("give requested.function"), "{err}");
    assert!(err.contains("item X1 is drawn as a valve"), "{err}");
    // A valve left out of the block but drawn between parts that are in it, and a typo far off.
    let mut outside = demo_circuit("demo-import-motors");
    outside["in_block"] = json!(["P", "T", "FC1", "FC2", "FC3", "RV1", "M1"]);
    outside["layout"]["components"]["HM1"]["x"] = json!(2e8);
    let e = mcp.import_circuit(import(outside)).await.unwrap_err();
    assert!(
        e.contains("V1 is not in in_block, but its layout puts it over"),
        "{e}"
    );
    assert!(e.contains("HM1 is placed at 200000000, 15"), "{e}");
    assert!(list_circuits(&pool).await.unwrap().is_empty());
    // A JSON string holding the object works, and a missing schema is filled in.
    let mut json = demo_circuit("demo-import-motors");
    json.as_object_mut().unwrap().remove("schema");
    let reply = mcp.import_circuit(import(json.to_string().into())).await;
    assert!(reply.is_ok(), "{reply:?}");
    let err = mcp.import_circuit(import(json!([1]))).await.unwrap_err();
    assert!(err.contains("must be a JSON object"), "{err}");
}
