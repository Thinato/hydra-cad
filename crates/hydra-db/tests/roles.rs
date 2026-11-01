//! An engineer's own login role, set up by ops/roles.sql, can do everything the app does.

use std::path::Path;

use hydra_core::{Catalogue, CatalogueRef, Circuit, Proposal};
use hydra_db::{
    ModelOwner, approve_proposal, attach_datasheet, attach_model, check_schema, create_circuit,
    import_catalogue, load_catalogue, save_revision, save_valve_type, submit_proposal,
};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

const ROLES: &str = include_str!("../../../ops/roles.sql");

fn repo_file(path: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .unwrap()
}

/// Connect to the test database as a new member of hydra_engineer.
async fn engineer(admin: &PgPool, name: &str) -> PgPool {
    sqlx::raw_sql(ROLES).execute(admin).await.unwrap();
    // Roles are cluster-wide: drop one a failed run left behind.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP ROLE IF EXISTS {name}; CREATE ROLE {name} LOGIN PASSWORD 'pilot' IN ROLE hydra_engineer"
    )))
    .execute(admin)
    .await
    .unwrap();
    let options = (*admin.connect_options())
        .clone()
        .username(name)
        .password("pilot");
    PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap()
}

async fn drop_role(admin: &PgPool, engineer: PgPool, name: &str) {
    engineer.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP ROLE {name}")))
        .execute(admin)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn engineers_role_does_what_the_app_does(admin: PgPool) {
    let name = "hydra_test_engineer";
    let pool = engineer(&admin, name).await;

    check_schema(&pool).await.unwrap();
    let fixture: Catalogue = serde_json::from_slice(&repo_file("fixtures/catalogue.json")).unwrap();
    import_catalogue(&pool, &fixture).await.unwrap();
    let mut catalogue = load_catalogue(&pool).await.unwrap().catalogue;
    let spool = CatalogueRef::new("vt-demo-4-2", 1);
    let mut edited = catalogue.valve_type(&spool).unwrap().clone();
    edited.code.push_str(" rev");
    let saved = save_valve_type(&pool, &edited).await.unwrap();
    attach_datasheet(&pool, &saved, b"%PDF-1.4 sheet")
        .await
        .unwrap();
    let glb = repo_file("fixtures/models/demo-valve-cetop3.glb");
    let owner = ModelOwner::ValveType(saved.clone());
    attach_model(&pool, &owner, &glb, None).await.unwrap();
    // Replacing a model deletes the old link.
    attach_model(
        &pool,
        &owner,
        &glb,
        Some(&repo_file("fixtures/models/demo-valve-cetop3.step")),
    )
    .await
    .unwrap();

    let circuit = Circuit::from_json(
        &String::from_utf8(repo_file("fixtures/circuits/demo-manifold.json")).unwrap(),
    )
    .unwrap();
    let opened = create_circuit(&pool, &circuit, None, Some("first"))
        .await
        .unwrap();
    save_revision(&pool, &opened.id, opened.revision, &circuit, None)
        .await
        .unwrap();
    let by: String = sqlx::query_scalar("SELECT created_by FROM circuit_revisions LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(by, name, "revisions record who saved them");

    catalogue = load_catalogue(&pool).await.unwrap().catalogue;
    let base = catalogue.valve_type(&saved).unwrap().clone();
    let proposal: Proposal = serde_json::from_value(json!({
        "valve_type": base,
        "parts": [{"part_number": "DEMO-4-2-PILOT"}]
    }))
    .unwrap();
    let id = submit_proposal(&pool, &proposal).await.unwrap();
    // submit_proposal refuses a second pending one, so queue the duplicate directly.
    let other: String = sqlx::query_scalar(
        "INSERT INTO catalogue_proposals (proposal) \
         SELECT proposal FROM catalogue_proposals WHERE id = $1::uuid RETURNING id::text",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    approve_proposal(&pool, &id, &proposal).await.unwrap();

    // Authors come from the signed-in role, whatever the client writes.
    for sql in [
        "INSERT INTO circuit_revisions (circuit_id, number, document, created_by) \
         SELECT circuit_id, 99, document, 'boss' FROM circuit_revisions LIMIT 1"
            .to_string(),
        "UPDATE circuits SET owner = 'boss'".to_string(),
        format!(
            "UPDATE catalogue_proposals SET status = 'rejected', reviewed_by = 'boss', \
             review_note = 'forged' WHERE id = '{other}'"
        ),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&pool)
            .await
            .unwrap();
    }
    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT created_by FROM circuit_revisions UNION ALL SELECT owner FROM circuits \
         UNION ALL SELECT reviewed_by FROM catalogue_proposals",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(authors.len(), 6);
    assert!(authors.iter().all(|a| a == name), "{authors:?}");
    // Stored files keep their bytes, even for the admin.
    let forge = "UPDATE assets SET bytes = '\\x00'";
    assert!(sqlx::raw_sql(forge).execute(&pool).await.is_err());
    let err = sqlx::raw_sql(forge).execute(&admin).await.unwrap_err();
    assert!(
        err.to_string().contains("stored files never change"),
        "{err}"
    );

    // The schema and its history stay the admin's.
    for sql in [
        "DELETE FROM _sqlx_migrations",
        "UPDATE _sqlx_migrations SET success = false",
        "TRUNCATE circuit_revisions",
        "DROP TABLE parts",
        "CREATE TABLE sneaky (id int)",
    ] {
        assert!(
            sqlx::raw_sql(sql).execute(&pool).await.is_err(),
            "an engineer could run {sql}"
        );
    }
    drop_role(&admin, pool, name).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn only_a_wrong_password_is_a_wrong_login(admin: PgPool) {
    let name = "hydra_test_retired";
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP ROLE IF EXISTS {name}; CREATE ROLE {name} NOLOGIN PASSWORD 'pilot'"
    )))
    .execute(&admin)
    .await
    .unwrap();
    let base = (*admin.connect_options()).clone();
    let attempt = |user: &str, password: &str| {
        hydra_db::connect_with(base.clone().username(user).password(password))
    };
    let wrong = attempt("postgres", "not-the-password").await.unwrap_err();
    assert!(wrong.is_wrong_login(), "{wrong}");
    // Retyping the password can't fix a role that may not log in (SQLSTATE 28000).
    let refused = attempt(name, "pilot").await.unwrap_err();
    assert!(!refused.is_wrong_login(), "{refused}");
    assert!(refused.is_login_refused(), "{refused}");
    assert!(!wrong.is_login_refused(), "{wrong}");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP ROLE {name}")))
        .execute(&admin)
        .await
        .unwrap();
}
