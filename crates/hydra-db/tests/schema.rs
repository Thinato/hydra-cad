use hydra_db::{Error, check_schema};
use sqlx::PgPool;

const TABLES: [&str; 10] = [
    "manufacturers",
    "symbol_defs",
    "valve_types",
    "parts",
    "blocks",
    "assets",
    "part_assets",
    "catalogue_proposals",
    "circuits",
    "circuit_revisions",
];

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn migrations_create_all_tables(pool: PgPool) {
    for t in TABLES {
        let found: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{t}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(found, "missing table {t}");
    }
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn check_schema_ok_when_migrated(pool: PgPool) {
    check_schema(&pool).await.unwrap();
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn check_schema_rejects_newer_database(pool: PgPool) {
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (999999, 'future', true, '\\x00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        check_schema(&pool).await,
        Err(Error::DatabaseTooNew { db: 999999, app: 1 })
    ));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn check_schema_rejects_edited_migration(pool: PgPool) {
    sqlx::query("UPDATE _sqlx_migrations SET checksum = '\\x00' WHERE version = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        check_schema(&pool).await,
        Err(Error::Modified { version: 1 })
    ));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn catalogue_version_is_unique_per_lineage(pool: PgPool) {
    let insert = "INSERT INTO symbol_defs (kind, definition, lineage_id, version) \
                  VALUES ('check', '{}', '00000000-0000-0000-0000-000000000001', $1)";
    sqlx::query(insert).bind(1).execute(&pool).await.unwrap();
    sqlx::query(insert).bind(2).execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).bind(2).execute(&pool).await.is_err());
}

#[sqlx::test(migrations = false)]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn check_schema_not_migrated(pool: PgPool) {
    assert!(matches!(check_schema(&pool).await, Err(Error::NotMigrated)));
}

/// Insert a circuit with revision 1; returns (circuit_id, revision_id).
async fn circuit_with_rev1(pool: &PgPool) -> (sqlx::types::Uuid, sqlx::types::Uuid) {
    let circuit: sqlx::types::Uuid =
        sqlx::query_scalar("INSERT INTO circuits (name) VALUES ('c') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    let rev: sqlx::types::Uuid = sqlx::query_scalar(
        "INSERT INTO circuit_revisions (circuit_id, number, document) \
         VALUES ($1, 1, '{}') RETURNING id",
    )
    .bind(circuit)
    .fetch_one(pool)
    .await
    .unwrap();
    (circuit, rev)
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn duplicate_revision_number_rejected(pool: PgPool) {
    let (circuit, _) = circuit_with_rev1(&pool).await;
    let dup = sqlx::query(
        "INSERT INTO circuit_revisions (circuit_id, number, document) VALUES ($1, 1, '{}')",
    )
    .bind(circuit)
    .execute(&pool)
    .await;
    assert!(dup.is_err());
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn restore_revision_references_earlier_one(pool: PgPool) {
    let (circuit, rev1) = circuit_with_rev1(&pool).await;
    sqlx::query(
        "INSERT INTO circuit_revisions (circuit_id, number, document, restored_from) \
         VALUES ($1, 2, '{}', $2)",
    )
    .bind(circuit)
    .bind(rev1)
    .execute(&pool)
    .await
    .unwrap();
}
