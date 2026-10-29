//! Hydra Postgres access: migrations, connection and schema-version check.

mod catalogue;

pub use catalogue::{
    ImportReport, LoadedCatalogue, asset_bytes, attach_datasheet, import_catalogue, load_catalogue,
    save_valve_type,
};
pub use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Embedded migrations from `./migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database has migrations this build does not know; update the app.
    #[error(
        "database schema (version {db}) is newer than this app (version {app}); update the app"
    )]
    DatabaseTooNew { db: i64, app: i64 },
    /// The database is empty or behind this build.
    #[error("database is not migrated; an admin must run migrations")]
    NotMigrated,
    /// An applied migration differs from the one this build carries.
    #[error(
        "database migration {version} differs from this app's copy; the schema was changed outside a release"
    )]
    Modified { version: i64 },
    /// The request breaks a catalogue rule; nothing was written.
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Lowercase sha256 hex of `bytes`: how assets are named.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Open a small connection pool.
pub async fn connect(url: &str) -> Result<PgPool, Error> {
    Ok(PgPoolOptions::new()
        .max_connections(4)
        // Fail fast: a desktop user is waiting to hear the database is unreachable.
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?)
}

/// Check that the database holds exactly this build's migrations, unchanged.
pub async fn check_schema(pool: &PgPool) -> Result<(), Error> {
    let app = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let has_table: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !has_table {
        return Err(Error::NotMigrated);
    }
    let applied: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version, checksum FROM _sqlx_migrations WHERE success ORDER BY version",
    )
    .fetch_all(pool)
    .await?;
    if let Some(&(db, _)) = applied.last()
        && db > app
    {
        return Err(Error::DatabaseTooNew { db, app });
    }
    for (version, checksum) in &applied {
        match MIGRATOR.iter().find(|m| m.version == *version) {
            Some(m) if *m.checksum == checksum[..] => {}
            _ => return Err(Error::Modified { version: *version }),
        }
    }
    if MIGRATOR
        .iter()
        .any(|m| !applied.iter().any(|(v, _)| *v == m.version))
    {
        return Err(Error::NotMigrated);
    }
    Ok(())
}
