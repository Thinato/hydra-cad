use std::path::Path;

use hydra_core::{Catalogue, CatalogueRef, PartDraft, Proposal, SymbolDefRecord};
use hydra_db::{
    Error, ProposalStatus, approve_proposal, attach_datasheet, get_proposal, import_catalogue,
    list_proposals, load_catalogue, reject_proposal, save_valve_type, submit_proposal,
};
use serde_json::json;
use sqlx::PgPool;

const PDF: &[u8] = b"%PDF-1.4 fake datasheet";
const NO_ID: &str = "00000000-0000-0000-0000-000000000000";

fn fixture() -> Catalogue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/catalogue.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A new demo valve type using an existing symbol, with two parts.
fn new_key() -> Proposal {
    serde_json::from_value(json!({
        "valve_type": {
            "key": "vt-demo-new", "version": 0, "manufacturer": "demo",
            "series": "DEMO-10", "code": "4-3-C", "symbol": "sym-4-3-closed-centre@1",
            "mounting": "ISO 4401-05",
            "params": {"function": "directional", "max_pressure_bar": 315}
        },
        "parts": [
            {"part_number": "DEMO-10-24DC", "options": {"voltage": "24 V DC"}},
            {"part_number": "DEMO-10-230AC", "options": {"voltage": "230 V AC"}}
        ],
        "notes": "page 1, ordering code table"
    }))
    .unwrap()
}

/// An update of fixture `vt-demo-4-2@1`: replaces part 24DC's options and adds a 12DC part.
fn update() -> Proposal {
    let f = fixture();
    let base = f
        .valve_types
        .iter()
        .find(|v| v.key == "vt-demo-4-2")
        .unwrap();
    Proposal {
        valve_type: base.clone(),
        new_symbol: None,
        parts: vec![
            PartDraft {
                part_number: "DEMO-06-4-2-S-24DC".into(),
                options: json!({"voltage": "24 V DC", "seals": "FKM"}),
            },
            PartDraft {
                part_number: "DEMO-06-4-2-S-12DC".into(),
                options: json!({"voltage": "12 V DC"}),
            },
        ],
        notes: String::new(),
    }
}

fn invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> Vec<String> {
    match r {
        Err(Error::Invalid(msgs)) => msgs,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn imported(pool: &PgPool) {
    import_catalogue(pool, &fixture()).await.unwrap();
}

async fn me(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn proposal_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_proposals")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `(part_number, options)` of each part of `r`.
fn parts(c: &Catalogue, r: &CatalogueRef) -> Vec<(String, serde_json::Value)> {
    c.parts_of(r)
        .map(|p| (p.part_number.clone(), p.options.clone()))
        .collect()
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn submitted_proposal_is_listed_and_found(pool: PgPool) {
    imported(&pool).await;
    let p = new_key();
    let id = submit_proposal(&pool, &p).await.unwrap();

    let pending = list_proposals(&pool, true).await.unwrap();
    assert_eq!(pending.len(), 1);
    let r = &pending[0];
    assert_eq!(r.id, id);
    assert_eq!(r.status, ProposalStatus::Pending);
    assert_eq!(r.proposal, p);
    assert_eq!(r.approved, None);
    assert_eq!(r.datasheet_sha256, None);
    assert_eq!(r.created_by, me(&pool).await);
    assert_eq!(r.created_at.len(), "YYYY-MM-DD HH:MI".len());
    assert_eq!(
        (&r.reviewed_by, &r.reviewed_at, &r.review_note),
        (&None, &None, &None)
    );
    assert_eq!(&get_proposal(&pool, &id).await.unwrap(), r);
    assert_eq!(list_proposals(&pool, false).await.unwrap(), pending);

    for bad in ["nope", NO_ID] {
        assert_eq!(
            invalid(get_proposal(&pool, bad).await),
            ["unknown proposal"]
        );
        assert_eq!(
            invalid(approve_proposal(&pool, bad, &p).await),
            ["unknown proposal"]
        );
        assert_eq!(
            invalid(reject_proposal(&pool, bad, "why").await),
            ["unknown proposal"]
        );
    }
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn invalid_proposal_is_refused_and_not_stored(pool: PgPool) {
    imported(&pool).await;
    let mut p = new_key();
    p.valve_type.version = 3;
    p.valve_type.code = " ".into();
    assert_eq!(
        invalid(submit_proposal(&pool, &p).await),
        [
            "valve type vt-demo-new doesn't exist yet; use version 0 to propose a new one",
            "the valve type needs a series and a code",
        ]
    );
    assert_eq!(proposal_count(&pool).await, 0);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn proposal_links_the_valve_types_datasheet(pool: PgPool) {
    imported(&pool).await;
    let p = update();
    let sha = attach_datasheet(&pool, &p.valve_type.reference(), PDF)
        .await
        .unwrap();
    let id = submit_proposal(&pool, &p).await.unwrap();
    let r = get_proposal(&pool, &id).await.unwrap();
    assert_eq!(r.datasheet_sha256, Some(sha));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn approving_a_new_key_creates_version_1(pool: PgPool) {
    imported(&pool).await;
    let p = new_key();
    let id = submit_proposal(&pool, &p).await.unwrap();
    let target = approve_proposal(&pool, &id, &p).await.unwrap();
    assert_eq!(target, CatalogueRef::new("vt-demo-new", 1));

    let c = load_catalogue(&pool).await.unwrap().catalogue;
    let saved = c.valve_type(&target).unwrap();
    assert_eq!(saved.series, "DEMO-10");
    assert_eq!(
        saved.params,
        json!({"function": "directional", "max_pressure_bar": 315})
    );
    assert_eq!(
        parts(&c, &target),
        [
            ("DEMO-10-230AC".into(), json!({"voltage": "230 V AC"})),
            ("DEMO-10-24DC".into(), json!({"voltage": "24 V DC"})),
        ]
    );

    let r = get_proposal(&pool, &id).await.unwrap();
    assert_eq!(r.status, ProposalStatus::Approved);
    assert_eq!(r.approved, Some(p));
    assert_eq!(r.reviewed_by, Some(me(&pool).await));
    assert!(r.reviewed_at.is_some());
    assert_eq!(r.review_note, None);
    assert!(list_proposals(&pool, true).await.unwrap().is_empty());
    assert_eq!(list_proposals(&pool, false).await.unwrap(), [r]);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn approving_an_update_keeps_and_replaces_parts_with_reviewer_edits(pool: PgPool) {
    imported(&pool).await;
    let p = update();
    let base = p.valve_type.reference();
    let sha = attach_datasheet(&pool, &base, PDF).await.unwrap();
    let id = submit_proposal(&pool, &p).await.unwrap();

    let mut edited = p.clone();
    edited.valve_type.series = "DEMO-06B".into();
    let target = approve_proposal(&pool, &id, &edited).await.unwrap();
    assert_eq!(target, CatalogueRef::new("vt-demo-4-2", 2));

    let loaded = load_catalogue(&pool).await.unwrap();
    let c = &loaded.catalogue;
    assert_eq!(c.valve_type(&target).unwrap().series, "DEMO-06B");
    assert_eq!(c.valve_type(&base).unwrap().series, "DEMO-06");
    assert_eq!(loaded.datasheets.get(&target), Some(&sha));
    let connector = "EN 175301-803";
    assert_eq!(
        parts(c, &target),
        [
            ("DEMO-06-4-2-S-12DC".into(), json!({"voltage": "12 V DC"})),
            (
                "DEMO-06-4-2-S-230AC".into(),
                json!({"voltage": "230 V AC", "connector": connector})
            ),
            (
                "DEMO-06-4-2-S-24DC".into(),
                json!({"voltage": "24 V DC", "seals": "FKM"})
            ),
        ]
    );
    assert_eq!(parts(c, &base).len(), 2);

    let r = get_proposal(&pool, &id).await.unwrap();
    assert_eq!(r.proposal, p);
    assert_eq!(r.approved, Some(edited));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn approving_saves_the_new_symbol(pool: PgPool) {
    imported(&pool).await;
    let mut p = new_key();
    let symbol = SymbolDefRecord {
        key: "sym-demo-new".into(),
        version: 1,
        definition: serde_json::from_value(json!({
            "kind": "directional_valve",
            "ports": [{"name": "A", "side": "top"}, {"name": "P", "side": "bottom"}],
            "positions": [{"flows": [["P", "A"]]}, {"blocked": ["P", "A"]}],
            "normal_position": 1
        }))
        .unwrap(),
    };
    p.new_symbol = Some(symbol.clone());
    p.valve_type.symbol = Some(symbol.reference());
    let id = submit_proposal(&pool, &p).await.unwrap();
    let target = approve_proposal(&pool, &id, &p).await.unwrap();

    let c = load_catalogue(&pool).await.unwrap().catalogue;
    assert!(c.symbol_defs.contains(&symbol));
    assert_eq!(
        c.valve_type(&target).unwrap().symbol,
        Some(symbol.reference())
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn approval_refuses_a_symbol_colliding_with_an_unreadable_one(pool: PgPool) {
    imported(&pool).await;
    sqlx::query(
        "INSERT INTO symbol_defs (key, version, kind, definition) \
         VALUES ('sym-future', 1, 'future_kind', '{\"kind\":\"future_kind\"}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut p = new_key();
    let symbol = SymbolDefRecord {
        key: "sym-future".into(),
        version: 1,
        definition: serde_json::from_value(json!({
            "kind": "directional_valve",
            "ports": [{"name": "A", "side": "top"}, {"name": "P", "side": "bottom"}],
            "positions": [{"flows": [["P", "A"]]}, {"blocked": ["P", "A"]}],
            "normal_position": 1
        }))
        .unwrap(),
    };
    p.new_symbol = Some(symbol.clone());
    p.valve_type.symbol = Some(symbol.reference());
    let id = submit_proposal(&pool, &p).await.unwrap();
    assert_eq!(
        invalid(approve_proposal(&pool, &id, &p).await),
        vec!["stored symbol sym-future@1 can't be read by this build; update the app"]
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn a_proposal_is_reviewed_once(pool: PgPool) {
    imported(&pool).await;
    let who = me(&pool).await;
    let p = new_key();
    let approved = submit_proposal(&pool, &p).await.unwrap();
    approve_proposal(&pool, &approved, &p).await.unwrap();
    let again = format!("proposal was already approved by {who}");
    assert_eq!(
        invalid(approve_proposal(&pool, &approved, &p).await),
        [again.as_str()]
    );
    assert_eq!(
        invalid(reject_proposal(&pool, &approved, "no").await),
        [again.as_str()]
    );

    let rejected = submit_proposal(&pool, &update()).await.unwrap();
    reject_proposal(&pool, &rejected, "wrong").await.unwrap();
    let again = format!("proposal was already rejected by {who}");
    assert_eq!(
        invalid(approve_proposal(&pool, &rejected, &update()).await),
        [again.as_str()]
    );
    assert_eq!(
        invalid(reject_proposal(&pool, &rejected, "no").await),
        [again.as_str()]
    );
    let c = load_catalogue(&pool).await.unwrap().catalogue;
    assert!(c.valve_type(&CatalogueRef::new("vt-demo-4-2", 2)).is_none());
    let ids: Vec<String> = list_proposals(&pool, false)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [rejected, approved], "newest first");
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn list_shows_at_most_200(pool: PgPool) {
    imported(&pool).await;
    submit_proposal(&pool, &new_key()).await.unwrap();
    sqlx::query(
        "INSERT INTO catalogue_proposals (proposal) \
         SELECT proposal FROM catalogue_proposals, generate_series(1, 200)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(proposal_count(&pool).await, 201);
    assert_eq!(list_proposals(&pool, true).await.unwrap().len(), 200);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn a_reviewer_cant_change_the_key(pool: PgPool) {
    imported(&pool).await;
    let p = new_key();
    let id = submit_proposal(&pool, &p).await.unwrap();
    let mut edited = p.clone();
    edited.valve_type.key = "vt-demo-other".into();
    assert_eq!(
        invalid(approve_proposal(&pool, &id, &edited).await),
        ["a reviewer can't change which valve type a proposal is for"]
    );
    let r = get_proposal(&pool, &id).await.unwrap();
    assert_eq!(r.status, ProposalStatus::Pending);
    let c = load_catalogue(&pool).await.unwrap().catalogue;
    assert!(
        c.valve_type(&CatalogueRef::new("vt-demo-other", 1))
            .is_none()
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn a_stale_base_makes_approval_invalid(pool: PgPool) {
    imported(&pool).await;
    let p = update();
    let id = submit_proposal(&pool, &p).await.unwrap();
    let mut other = p.valve_type.clone();
    other.code = "4-2-S-X".into();
    save_valve_type(&pool, &other).await.unwrap();

    assert_eq!(
        invalid(approve_proposal(&pool, &id, &p).await),
        ["valve type vt-demo-4-2 is at version 2; base the proposal on version 2"]
    );
    assert_eq!(
        get_proposal(&pool, &id).await.unwrap().status,
        ProposalStatus::Pending
    );
    let c = load_catalogue(&pool).await.unwrap().catalogue;
    assert_eq!(
        c.valve_type(&CatalogueRef::new("vt-demo-4-2", 2))
            .unwrap()
            .code,
        "4-2-S-X"
    );
    assert!(c.valve_type(&CatalogueRef::new("vt-demo-4-2", 3)).is_none());
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn rejecting_needs_a_note(pool: PgPool) {
    imported(&pool).await;
    let id = submit_proposal(&pool, &new_key()).await.unwrap();
    for note in ["", "  \n"] {
        assert_eq!(
            invalid(reject_proposal(&pool, &id, note).await),
            ["say why, so the assistant can fix the proposal"]
        );
    }
    reject_proposal(&pool, &id, " wrong voltage ")
        .await
        .unwrap();
    let r = get_proposal(&pool, &id).await.unwrap();
    assert_eq!(r.status, ProposalStatus::Rejected);
    assert_eq!(r.review_note.as_deref(), Some("wrong voltage"));
    assert_eq!(r.reviewed_by, Some(me(&pool).await));
    assert!(r.reviewed_at.is_some());
    assert_eq!(r.approved, None);
    assert!(list_proposals(&pool, true).await.unwrap().is_empty());
}

/// A review that commits while an approval waits for the proposal row.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn concurrent_review_makes_approval_invalid(pool: PgPool) {
    imported(&pool).await;
    let p = new_key();
    let id = submit_proposal(&pool, &p).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE catalogue_proposals SET status = 'rejected', reviewed_by = 'other' \
         WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let approval = tokio::spawn({
        let (pool, id, p) = (pool.clone(), id.clone(), p.clone());
        async move { approve_proposal(&pool, &id, &p).await }
    });
    let waiting = "SELECT count(*) FROM pg_stat_activity \
                   WHERE datname = current_database() AND wait_event_type = 'Lock'";
    while sqlx::query_scalar::<_, i64>(waiting)
        .fetch_one(&pool)
        .await
        .unwrap()
        == 0
    {
        tokio::task::yield_now().await;
    }
    tx.commit().await.unwrap();
    assert_eq!(
        invalid(approval.await.unwrap()),
        ["someone reviewed this proposal meanwhile; reload it"]
    );
}

async fn refused(pool: &PgPool, sql: &'static str, id: &str) {
    let err = sqlx::query(sql).bind(id).execute(pool).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("proposals are never edited or deleted"),
        "{sql}: {err}"
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn reviewed_proposals_are_history(pool: PgPool) {
    imported(&pool).await;
    let pending = submit_proposal(&pool, &update()).await.unwrap();
    let reviewed = submit_proposal(&pool, &new_key()).await.unwrap();
    reject_proposal(&pool, &reviewed, "no").await.unwrap();

    let reopen = "UPDATE catalogue_proposals SET status = 'pending', reviewed_by = NULL \
                  WHERE id = $1::uuid";
    refused(&pool, reopen, &reviewed).await;
    let renote = "UPDATE catalogue_proposals SET review_note = 'x' WHERE id = $1::uuid";
    refused(&pool, renote, &reviewed).await;
    let delete = "DELETE FROM catalogue_proposals WHERE id = $1::uuid";
    refused(&pool, delete, &reviewed).await;
    refused(&pool, delete, &pending).await;
    // Reviewing must not change what was submitted.
    let rewrite = "UPDATE catalogue_proposals \
                   SET status = 'approved', reviewed_by = 'x', proposal = '{}' \
                   WHERE id = $1::uuid";
    refused(&pool, rewrite, &pending).await;
    // The reviewer is whoever reviews, whatever the statement says.
    let unsigned = "UPDATE catalogue_proposals SET status = 'approved', reviewed_by = 'boss' \
                    WHERE id = $1::uuid";
    sqlx::query(unsigned)
        .bind(&pending)
        .execute(&pool)
        .await
        .unwrap();
    let by = get_proposal(&pool, &pending).await.unwrap().reviewed_by;
    assert_eq!(by, Some(me(&pool).await));
    assert_eq!(proposal_count(&pool).await, 2);
    let r = get_proposal(&pool, &reviewed).await.unwrap();
    assert_eq!(r.review_note.as_deref(), Some("no"));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn new_proposals_start_pending_and_unreadable_ones_are_skipped(pool: PgPool) {
    imported(&pool).await;
    // History can't be written as already reviewed, nor under someone else's name.
    let insert = "INSERT INTO catalogue_proposals (proposal, status, reviewed_by) \
                  VALUES ('{}', $1, $2)";
    for (status, by) in [
        ("pending", Some("x")),
        ("approved", Some("x")),
        ("rejected", None),
    ] {
        let err = sqlx::query(insert)
            .bind(status)
            .bind(by)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("starts pending"), "{err}");
    }
    let by: String = sqlx::query_scalar(
        "INSERT INTO catalogue_proposals (proposal, created_by) VALUES ('{}', 'someone') \
         RETURNING created_by",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(by, me(&pool).await);
    let err = sqlx::query("TRUNCATE catalogue_proposals")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("never edited or deleted"), "{err}");

    // That '{}' row isn't a proposal this build can read; the queue still lists the rest.
    let id = submit_proposal(&pool, &new_key()).await.unwrap();
    let ids: Vec<_> = list_proposals(&pool, true)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [id]);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn the_queue_is_oldest_first_and_oversized_proposals_are_refused(pool: PgPool) {
    imported(&pool).await;
    let first = submit_proposal(&pool, &new_key()).await.unwrap();
    let second = submit_proposal(&pool, &update()).await.unwrap();
    let ids: Vec<_> = list_proposals(&pool, true)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(
        ids,
        [first.clone(), second.clone()],
        "pending: oldest first"
    );
    let ids: Vec<_> = list_proposals(&pool, false)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [second, first], "all: newest first");

    let mut big = new_key();
    big.valve_type.params = serde_json::json!({"blob": "x".repeat(hydra_db::MAX_PROPOSAL_BYTES)});
    let problems = invalid(submit_proposal(&pool, &big).await);
    assert!(problems[0].contains("at most 256 KiB"), "{problems:?}");
}

/// The id of the pending proposal that refused `r`, after checking the refusal's message.
fn already_pending<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    let e = r.expect_err("expected AlreadyPending");
    let Error::AlreadyPending { id } = &e else {
        panic!("expected AlreadyPending, got {e:?}");
    };
    assert_eq!(
        e.to_string(),
        format!("proposal {id} for the same valve type version is still waiting for review")
    );
    id.clone()
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn one_pending_proposal_per_valve_type_version(pool: PgPool) {
    imported(&pool).await;
    let first = submit_proposal(&pool, &update()).await.unwrap();
    let mut other = update();
    other.notes = "a second try".into();
    assert_eq!(already_pending(submit_proposal(&pool, &other).await), first);
    assert_eq!(proposal_count(&pool).await, 1);

    // Another valve type is unaffected.
    submit_proposal(&pool, &new_key()).await.unwrap();

    // Once rejected, a new one is queued.
    reject_proposal(&pool, &first, "wrong seals").await.unwrap();
    let second = submit_proposal(&pool, &other).await.unwrap();

    // A newer base version is a different proposal; the stale one stays reviewable.
    let mut v = update().valve_type;
    v.code = "4-2-S-X".into();
    save_valve_type(&pool, &v).await.unwrap();
    let mut newer = update();
    newer.valve_type = v;
    newer.valve_type.version = 2;
    submit_proposal(&pool, &newer).await.unwrap();
    assert_eq!(
        get_proposal(&pool, &second).await.unwrap().status,
        ProposalStatus::Pending
    );
    assert_eq!(proposal_count(&pool).await, 4);
}

/// Two submissions racing for the same valve type version: one is queued, one refused.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn concurrent_duplicates_queue_only_one(pool: PgPool) {
    imported(&pool).await;
    // Hold both submitters at the start line, then release them together.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('hydra proposal: vt-demo-4-2'))")
        .execute(&mut *tx)
        .await
        .unwrap();
    let submit = || {
        let pool = pool.clone();
        tokio::spawn(async move { submit_proposal(&pool, &update()).await })
    };
    let (a, b) = (submit(), submit());
    let waiting = "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
                   AND database = (SELECT oid FROM pg_database WHERE datname = current_database())";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sqlx::query_scalar::<_, i64>(waiting)
        .fetch_one(&pool)
        .await
        .unwrap()
        < 2
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    tx.commit().await.unwrap();
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    let (queued, refused) = if a.is_ok() { (a, b) } else { (b, a) };
    assert_eq!(already_pending(refused), queued.unwrap());
    assert_eq!(proposal_count(&pool).await, 1);
}
