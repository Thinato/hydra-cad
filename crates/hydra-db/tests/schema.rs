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
        Err(Error::DatabaseTooNew { db: 999999, app: 5 })
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
async fn catalogue_version_is_unique_per_key(pool: PgPool) {
    let insert = "INSERT INTO symbol_defs (kind, definition, key, version) \
                  VALUES ('check', '{}', 'sym-check', $1)";
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

/// Insert a manufacturer, symbol `sym@1`, valve type `vt@1` and one part.
async fn catalogue_rows(pool: &PgPool) {
    sqlx::raw_sql(
        "INSERT INTO manufacturers (key, name) VALUES ('m', 'M'); \
         INSERT INTO symbol_defs (kind, definition, key) VALUES ('check', '{}', 'sym'); \
         INSERT INTO valve_types (manufacturer_id, series, code, symbol_def_id, key) \
         SELECT m.id, 'S', 'C', s.id, 'vt' FROM manufacturers m, symbol_defs s; \
         INSERT INTO parts (valve_type_id, part_number) SELECT id, 'P1' FROM valve_types;",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn refused(pool: &PgPool, sql: &'static str) {
    let err = sqlx::query(sql).execute(pool).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("catalogue rows are never edited in place; add a new version"),
        "{sql}: {err}"
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn catalogue_rows_are_immutable(pool: PgPool) {
    catalogue_rows(&pool).await;
    refused(&pool, "UPDATE valve_types SET code = 'X'").await;
    refused(&pool, "DELETE FROM valve_types").await;
    refused(&pool, "DELETE FROM symbol_defs").await;
    refused(&pool, "UPDATE symbol_defs SET kind = 'x'").await;
    refused(&pool, "UPDATE parts SET part_number = 'P2'").await;
    refused(&pool, "DELETE FROM parts").await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn blocks_are_immutable(pool: PgPool) {
    sqlx::query("INSERT INTO blocks (name, key) VALUES ('B', 'blk')")
        .execute(&pool)
        .await
        .unwrap();
    refused(&pool, "UPDATE blocks SET name = 'X'").await;
    refused(&pool, "DELETE FROM blocks").await;
    let not_array = "INSERT INTO blocks (name, key, mount_faces) VALUES ('B', 'blk2', '{}')";
    assert!(sqlx::query(not_array).execute(&pool).await.is_err());
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn one_display_model_per_owner(pool: PgPool) {
    sqlx::raw_sql(
        "INSERT INTO assets (sha256, kind, bytes, size) VALUES \
             (repeat('a', 64), 'gltf', '\\x00', 1), (repeat('b', 64), 'gltf', '\\x01', 1); \
         INSERT INTO part_assets (owner_id, asset_id, role) \
         SELECT '00000000-0000-0000-0000-000000000001', id, 'display' \
         FROM assets WHERE sha256 = repeat('a', 64);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let second = sqlx::query(
        "INSERT INTO part_assets (owner_id, asset_id, role) \
         SELECT '00000000-0000-0000-0000-000000000001', id, 'display' \
         FROM assets WHERE sha256 = repeat('b', 64)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        second
            .as_database_error()
            .is_some_and(|d| d.is_unique_violation()),
        "{second}"
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn valve_type_datasheet_can_be_linked(pool: PgPool) {
    catalogue_rows(&pool).await;
    let updated = sqlx::query(
        "WITH a AS (INSERT INTO assets (sha256, kind, bytes, size) \
                    VALUES (repeat('a', 64), 'pdf', '\\x00', 1) RETURNING id) \
         UPDATE valve_types SET datasheet_asset_id = (SELECT id FROM a)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn asset_sha256_must_be_lowercase_hex(pool: PgPool) {
    let insert = "INSERT INTO assets (sha256, kind, bytes, size) VALUES ($1, 'pdf', '\\x00', 1)";
    for bad in ["A".repeat(64), "a".repeat(63)] {
        assert!(sqlx::query(insert).bind(bad).execute(&pool).await.is_err());
    }
    sqlx::query(insert)
        .bind("0".repeat(64))
        .execute(&pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn catalogue_keys_are_checked(pool: PgPool) {
    let insert = "INSERT INTO manufacturers (key, name) VALUES ($1, $1)";
    for bad in ["", " m", "m@1"] {
        assert!(sqlx::query(insert).bind(bad).execute(&pool).await.is_err());
    }
    sqlx::query(insert).bind("m").execute(&pool).await.unwrap();
}
