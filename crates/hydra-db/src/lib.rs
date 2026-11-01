//! Hydra Postgres access: migrations, connection and schema-version check.

mod catalogue;
mod circuits;
mod proposals;
mod users;

pub use catalogue::{
    ImportReport, LoadedCatalogue, ModelOwner, asset_bytes, attach_datasheet, attach_model,
    import_catalogue, load_catalogue, save_valve_type,
};
pub use circuits::{
    CircuitSummary, OpenedCircuit, RevisionSummary, create_circuit, create_circuit_once,
    list_circuits, list_revisions, open_circuit, open_revision, restore_revision, save_revision,
};
pub use proposals::{
    MAX_PROPOSAL_BYTES, ProposalRecord, ProposalStatus, approve_proposal, get_proposal,
    list_proposals, reject_proposal, submit_proposal,
};
pub use sqlx::PgPool;
pub use sqlx::postgres::PgConnectOptions;
use sqlx::postgres::PgPoolOptions;
pub use users::{
    Account, MIN_PASSWORD_CHARS, Session, User, account, create_user, delete_user, end_sessions,
    list_users, set_admin, set_can_login, set_password,
};

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
    /// The request breaks a rule or names something unknown; nothing was written.
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
    /// Someone saved the circuit after the caller opened it; nothing was written.
    /// `current` is the circuit's revision number now.
    #[error("someone saved revision {current} since you opened this circuit")]
    Conflict { current: i32 },
    /// Proposal `id`, for the same valve type and base version, still waits for review.
    #[error("proposal {id} for the same valve type version is still waiting for review")]
    AlreadyPending { id: String },
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl Error {
    /// Postgres refused the user name or password (SQLSTATE 28P01, which password
    /// authentication also gives for an unknown user). Not 28000: that is the server's
    /// pg_hba.conf or a role that may not log in, which the user can't fix by retyping.
    pub fn is_wrong_login(&self) -> bool {
        matches!(self, Error::Sqlx(sqlx::Error::Database(e)) if e.code().as_deref() == Some("28P01"))
    }

    /// Postgres won't let this role in at all (SQLSTATE 28000): it was disabled, or the
    /// server's pg_hba.conf doesn't allow it.
    pub fn is_login_refused(&self) -> bool {
        matches!(self, Error::Sqlx(sqlx::Error::Database(e)) if e.code().as_deref() == Some("28000"))
    }
}

/// Lowercase sha256 hex of `bytes`: how assets are named.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Open a small connection pool.
pub async fn connect(url: &str) -> Result<PgPool, Error> {
    connect_with(url.parse()?).await
}

/// [`connect`] with options built in code, e.g. a user name and password added to a URL.
pub async fn connect_with(options: PgConnectOptions) -> Result<PgPool, Error> {
    Ok(PgPoolOptions::new()
        .max_connections(4)
        // Fail fast: a desktop user is waiting to hear the database is unreachable.
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
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
