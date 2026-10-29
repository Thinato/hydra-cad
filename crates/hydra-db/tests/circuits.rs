use std::path::Path;

use hydra_core::Circuit;
use hydra_db::{
    Error, create_circuit, list_circuits, list_revisions, open_circuit, open_revision,
    restore_revision, save_revision,
};
use sqlx::PgPool;

fn demo() -> Circuit {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/circuits/demo-manifold.json");
    Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn renamed(name: &str) -> Circuit {
    Circuit {
        name: name.into(),
        ..demo()
    }
}

fn invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> Vec<String> {
    match r {
        Err(Error::Invalid(msgs)) => msgs,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn revision_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM circuit_revisions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn create_then_open_round_trips(pool: PgPool) {
    let doc = demo();
    let created = create_circuit(&pool, &doc, Some("Germani"), Some("first"))
        .await
        .unwrap();
    assert_eq!(created.revision, 1);
    let opened = open_circuit(&pool, &created.id).await.unwrap();
    assert_eq!(opened, created);
    assert_eq!(opened.document, doc);
    assert_eq!(opened.project.as_deref(), Some("Germani"));
    assert_eq!(open_revision(&pool, &created.id, 1).await.unwrap(), doc);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_adds_revision_and_lists_newest_first(pool: PgPool) {
    let c = create_circuit(&pool, &demo(), None, Some("first"))
        .await
        .unwrap();
    let second = renamed("second");
    assert_eq!(
        save_revision(&pool, &c.id, 1, &second, Some("rename"))
            .await
            .unwrap(),
        2
    );
    let opened = open_circuit(&pool, &c.id).await.unwrap();
    assert_eq!((opened.revision, &opened.document), (2, &second));
    assert_eq!(open_revision(&pool, &c.id, 1).await.unwrap(), demo());

    let revs = list_revisions(&pool, &c.id).await.unwrap();
    let got: Vec<_> = revs
        .iter()
        .map(|r| (r.number, r.message.as_deref(), r.restored_from))
        .collect();
    assert_eq!(got, [(2, Some("rename"), None), (1, Some("first"), None)]);
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(revs.iter().all(|r| r.created_by == role));
    assert!(
        revs.iter()
            .all(|r| r.created_at.len() == "YYYY-MM-DD HH:MI".len())
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_from_stale_base_conflicts(pool: PgPool) {
    let c = create_circuit(&pool, &demo(), None, None).await.unwrap();
    save_revision(&pool, &c.id, 1, &renamed("theirs"), None)
        .await
        .unwrap();
    let r = save_revision(&pool, &c.id, 1, &renamed("mine"), None).await;
    match r {
        Err(e @ Error::Conflict { current: 2 }) => assert_eq!(
            e.to_string(),
            "someone saved revision 2 since you opened this circuit"
        ),
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(revision_count(&pool).await, 2);
    let opened = open_circuit(&pool, &c.id).await.unwrap();
    assert_eq!(
        (opened.revision, opened.document.name.as_str()),
        (2, "theirs")
    );
    assert_eq!(list_circuits(&pool).await.unwrap()[0].name, "theirs");
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn restore_copies_old_revision_as_new_one(pool: PgPool) {
    let c = create_circuit(&pool, &demo(), None, None).await.unwrap();
    save_revision(&pool, &c.id, 1, &renamed("second"), None)
        .await
        .unwrap();
    assert_eq!(restore_revision(&pool, &c.id, 1, 2).await.unwrap(), 3);
    let opened = open_circuit(&pool, &c.id).await.unwrap();
    assert_eq!(opened.revision, 3);
    assert_eq!(
        opened.document,
        open_revision(&pool, &c.id, 1).await.unwrap()
    );
    let latest = &list_revisions(&pool, &c.id).await.unwrap()[0];
    assert_eq!(latest.number, 3);
    assert_eq!(latest.restored_from, Some(1));
    assert_eq!(latest.message.as_deref(), Some("Restored revision 1"));
    // The name follows the restored document.
    assert_eq!(list_circuits(&pool).await.unwrap()[0].name, demo().name);
    // Revision 2 is still there.
    assert_eq!(open_revision(&pool, &c.id, 2).await.unwrap().name, "second");
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn restore_from_stale_base_or_unknown_revision_is_refused(pool: PgPool) {
    let c = create_circuit(&pool, &demo(), None, None).await.unwrap();
    save_revision(&pool, &c.id, 1, &renamed("second"), None)
        .await
        .unwrap();
    assert!(matches!(
        restore_revision(&pool, &c.id, 1, 1).await,
        Err(Error::Conflict { current: 2 })
    ));
    assert_eq!(
        invalid(restore_revision(&pool, &c.id, 9, 2).await),
        ["unknown revision 9"]
    );
    assert_eq!(
        invalid(open_revision(&pool, &c.id, 9).await),
        ["unknown revision 9"]
    );
    assert_eq!(revision_count(&pool).await, 2);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn list_circuits_most_recently_saved_first(pool: PgPool) {
    let a = create_circuit(&pool, &renamed("a"), Some("P1"), None)
        .await
        .unwrap();
    let b = create_circuit(&pool, &renamed("b"), None, None)
        .await
        .unwrap();
    let ids: Vec<_> = list_circuits(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, [b.id.clone(), a.id.clone()]);

    save_revision(&pool, &a.id, 1, &renamed("a renamed"), None)
        .await
        .unwrap();
    let list = list_circuits(&pool).await.unwrap();
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    let first = &list[0];
    assert_eq!(first.id, a.id);
    assert_eq!(first.name, "a renamed");
    assert_eq!(first.project.as_deref(), Some("P1"));
    assert_eq!((first.revision, first.owner.as_str()), (2, role.as_str()));
    assert_eq!(first.saved_by, role);
    let now: String = sqlx::query_scalar("SELECT to_char(now(), 'YYYY-MM-DD')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(first.saved_at.starts_with(&now), "{}", first.saved_at);
    assert_eq!(first.saved_at.len(), "YYYY-MM-DD HH:MI".len());
    assert_eq!(
        (list[1].id.as_str(), list[1].name.as_str(), list[1].revision),
        (b.id.as_str(), "b", 1)
    );
    assert_eq!(list[1].project, None);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn unknown_or_garbage_ids_are_invalid(pool: PgPool) {
    let doc = demo();
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "not-a-uuid",
        "'; DROP TABLE circuits; --",
        "",
    ] {
        let unknown = ["unknown circuit"];
        assert_eq!(invalid(open_circuit(&pool, id).await), unknown, "{id}");
        assert_eq!(invalid(open_revision(&pool, id, 1).await), unknown);
        assert_eq!(invalid(list_revisions(&pool, id).await), unknown);
        assert_eq!(
            invalid(save_revision(&pool, id, 1, &doc, None).await),
            unknown
        );
        assert_eq!(invalid(restore_revision(&pool, id, 1, 1).await), unknown);
    }
    assert!(list_circuits(&pool).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn revisions_cannot_be_updated_or_deleted(pool: PgPool) {
    create_circuit(&pool, &demo(), None, None).await.unwrap();
    for sql in [
        "UPDATE circuit_revisions SET message = 'rewritten'",
        "DELETE FROM circuit_revisions",
    ] {
        let err = sqlx::query(sql).execute(&pool).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("circuit revisions are never changed; restore creates a new revision"),
            "{sql}: {err}"
        );
    }
    assert_eq!(revision_count(&pool).await, 1);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn unsupported_schema_is_invalid(pool: PgPool) {
    let id = create_circuit(&pool, &demo(), None, None).await.unwrap().id;
    sqlx::query(
        "WITH r AS (INSERT INTO circuit_revisions (circuit_id, number, document) \
                    VALUES ($1::uuid, 2, '{\"schema\": 2, \"name\": \"future\"}') RETURNING id) \
         UPDATE circuits SET current_revision_id = (SELECT id FROM r) WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    let msgs = invalid(open_circuit(&pool, &id).await);
    assert_eq!(
        msgs,
        ["circuit schema version 2 is not supported, this build reads version 1"]
    );
    invalid(open_revision(&pool, &id, 2).await);
    // And such a document is never written by this build.
    let mut future = demo();
    future.schema = 2;
    invalid(create_circuit(&pool, &future, None, None).await);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn concurrent_saves_from_same_base_one_wins(pool: PgPool) {
    let c = create_circuit(&pool, &demo(), None, None).await.unwrap();
    let (x, y) = (renamed("x"), renamed("y"));
    let (rx, ry) = tokio::join!(
        save_revision(&pool, &c.id, 1, &x, None),
        save_revision(&pool, &c.id, 1, &y, None)
    );
    let (winner, loser) = match (rx, ry) {
        (Ok(2), loser) => (x, loser),
        (loser, Ok(2)) => (y, loser),
        other => panic!("expected one save to win: {other:?}"),
    };
    assert!(
        matches!(loser, Err(Error::Conflict { current: 2 })),
        "{loser:?}"
    );
    assert_eq!(revision_count(&pool).await, 2);
    assert_eq!(open_circuit(&pool, &c.id).await.unwrap().document, winner);
}
