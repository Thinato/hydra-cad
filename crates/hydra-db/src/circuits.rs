//! Circuit repository: each save inserts a new immutable revision; the `circuits` row only
//! holds metadata and points at the current revision.
//!
//! Circuit ids are uuids passed as text. Revisions are found by their number.

use hydra_core::{Circuit, SCHEMA_VERSION};
use sqlx::{PgConnection, PgPool, types::Json, types::Uuid};

use crate::Error;

/// One row of the circuit list.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CircuitSummary {
    /// The circuit's uuid as text.
    pub id: String,
    /// The name of the current revision's document.
    pub name: String,
    pub project: Option<String>,
    /// Postgres role that created the circuit.
    pub owner: String,
    /// Number of the current revision.
    pub revision: i32,
    /// Postgres role that saved the current revision.
    pub saved_by: String,
    /// When the current revision was saved, `YYYY-MM-DD HH24:MI` in the session time zone.
    pub saved_at: String,
}

/// One revision of a circuit's history.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RevisionSummary {
    pub number: i32,
    /// Postgres role that saved it.
    pub created_by: String,
    /// `YYYY-MM-DD HH24:MI` in the session time zone.
    pub created_at: String,
    pub message: Option<String>,
    /// For a restore: the number of the revision it copied.
    pub restored_from: Option<i32>,
}

/// A circuit opened for editing. Pass `revision` back as `base_revision` when saving.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedCircuit {
    pub id: String,
    pub project: Option<String>,
    pub revision: i32,
    pub document: Circuit,
}

fn unknown_circuit() -> Error {
    Error::Invalid(vec!["unknown circuit".into()])
}

/// Bound as a uuid, so a malformed id is refused here and never reaches the SQL text.
fn circuit_uuid(id: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(id).map_err(|_| unknown_circuit())
}

/// Parse a stored document, checking its schema version.
fn parse(json: &str) -> Result<Circuit, Error> {
    Circuit::from_json(json).map_err(|e| Error::Invalid(vec![e.to_string()]))
}

/// Refuse to store a document this build could not open again.
fn check_writable(document: &Circuit) -> Result<(), Error> {
    if document.schema != SCHEMA_VERSION {
        return Err(Error::Invalid(vec![format!(
            "circuit schema version {} is not supported, this build writes version {SCHEMA_VERSION}",
            document.schema
        )]));
    }
    Ok(())
}

/// Every circuit with its current revision, most recently saved first.
pub async fn list_circuits(pool: &PgPool) -> Result<Vec<CircuitSummary>, Error> {
    Ok(sqlx::query_as(
        "SELECT c.id::text AS id, c.name, c.project, c.owner, r.number AS revision, \
                r.created_by AS saved_by, to_char(r.created_at, 'YYYY-MM-DD HH24:MI') AS saved_at \
         FROM circuits c JOIN circuit_revisions r ON r.id = c.current_revision_id \
         ORDER BY r.created_at DESC, c.id",
    )
    .fetch_all(pool)
    .await?)
}

/// Create a circuit whose revision 1 is `document`.
pub async fn create_circuit(
    pool: &PgPool,
    document: &Circuit,
    project: Option<&str>,
    message: Option<&str>,
) -> Result<OpenedCircuit, Error> {
    check_writable(document)?;
    let mut tx = pool.begin().await?;
    let opened = insert_circuit(&mut tx, document, project, message).await?;
    tx.commit().await?;
    Ok(opened)
}

/// Like [`create_circuit`], but refuses a name another circuit already has. Concurrent calls
/// with the same name are serialised, so a retried import can't create it twice.
pub async fn create_circuit_once(
    pool: &PgPool,
    document: &Circuit,
    message: Option<&str>,
) -> Result<OpenedCircuit, Error> {
    check_writable(document)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('hydra circuit name: ' || $1))")
        .bind(&document.name)
        .execute(&mut *tx)
        .await?;
    let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM circuits WHERE name = $1)")
        .bind(&document.name)
        .fetch_one(&mut *tx)
        .await?;
    if taken {
        return Err(Error::Invalid(vec![format!(
            "a circuit named {:?} already exists",
            document.name
        )]));
    }
    let opened = insert_circuit(&mut tx, document, None, message).await?;
    tx.commit().await?;
    Ok(opened)
}

async fn insert_circuit(
    tx: &mut PgConnection,
    document: &Circuit,
    project: Option<&str>,
    message: Option<&str>,
) -> Result<OpenedCircuit, Error> {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO circuits (name, project) VALUES ($1, $2) RETURNING id")
            .bind(&document.name)
            .bind(project)
            .fetch_one(&mut *tx)
            .await?;
    let revision: Uuid = sqlx::query_scalar(
        "INSERT INTO circuit_revisions (circuit_id, number, document, message) \
         VALUES ($1, 1, $2, $3) RETURNING id",
    )
    .bind(id)
    .bind(Json(document))
    .bind(message)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE circuits SET current_revision_id = $2 WHERE id = $1")
        .bind(id)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
    Ok(OpenedCircuit {
        id: id.to_string(),
        project: project.map(Into::into),
        revision: 1,
        document: document.clone(),
    })
}

/// Lock the circuit row and check that its current revision is still `base_revision`.
/// Returns the circuit's uuid and the number the next revision gets.
async fn lock_base(
    conn: &mut PgConnection,
    circuit_id: &str,
    base_revision: i32,
) -> Result<(Uuid, i32), Error> {
    let id = circuit_uuid(circuit_id)?;
    let current_id: Uuid =
        sqlx::query_scalar("SELECT current_revision_id FROM circuits WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(unknown_circuit)?;
    // A separate statement: under READ COMMITTED it sees a revision committed by a save we
    // waited on for the lock, which a join in the locking statement would miss.
    let current: i32 = sqlx::query_scalar("SELECT number FROM circuit_revisions WHERE id = $1")
        .bind(current_id)
        .fetch_one(&mut *conn)
        .await?;
    if current != base_revision {
        return Err(Error::Conflict { current });
    }
    Ok((id, current + 1))
}

/// Make `revision` the current one, and mirror its document's name.
async fn set_current(
    conn: &mut PgConnection,
    id: Uuid,
    revision: Uuid,
    name: &str,
) -> Result<(), Error> {
    sqlx::query("UPDATE circuits SET current_revision_id = $2, name = $3 WHERE id = $1")
        .bind(id)
        .bind(revision)
        .bind(name)
        .execute(conn)
        .await?;
    Ok(())
}

/// Save `document` as a new revision on top of `base_revision`, the revision it was opened
/// at. Returns the new revision's number, or [`Error::Conflict`] if someone saved meanwhile.
pub async fn save_revision(
    pool: &PgPool,
    circuit_id: &str,
    base_revision: i32,
    document: &Circuit,
    message: Option<&str>,
) -> Result<i32, Error> {
    check_writable(document)?;
    let mut tx = pool.begin().await?;
    let (id, next) = lock_base(&mut tx, circuit_id, base_revision).await?;
    let revision: Uuid = sqlx::query_scalar(
        "INSERT INTO circuit_revisions (circuit_id, number, document, message) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(id)
    .bind(next)
    .bind(Json(document))
    .bind(message)
    .fetch_one(&mut *tx)
    .await?;
    set_current(&mut tx, id, revision, &document.name).await?;
    tx.commit().await?;
    Ok(next)
}

/// The circuit at its current revision.
pub async fn open_circuit(pool: &PgPool, circuit_id: &str) -> Result<OpenedCircuit, Error> {
    let (id, project, revision, document): (String, Option<String>, i32, String) = sqlx::query_as(
        "SELECT c.id::text, c.project, r.number, r.document::text \
             FROM circuits c JOIN circuit_revisions r ON r.id = c.current_revision_id \
             WHERE c.id = $1",
    )
    .bind(circuit_uuid(circuit_id)?)
    .fetch_optional(pool)
    .await?
    .ok_or_else(unknown_circuit)?;
    Ok(OpenedCircuit {
        id,
        project,
        revision,
        document: parse(&document)?,
    })
}

/// The document of revision `number`.
pub async fn open_revision(pool: &PgPool, circuit_id: &str, number: i32) -> Result<Circuit, Error> {
    let document: Option<String> = sqlx::query_scalar(
        "SELECT r.document::text FROM circuits c \
         LEFT JOIN circuit_revisions r ON r.circuit_id = c.id AND r.number = $2 \
         WHERE c.id = $1",
    )
    .bind(circuit_uuid(circuit_id)?)
    .bind(number)
    .fetch_optional(pool)
    .await?
    .ok_or_else(unknown_circuit)?;
    let document =
        document.ok_or_else(|| Error::Invalid(vec![format!("unknown revision {number}")]))?;
    parse(&document)
}

/// The circuit's revisions, newest first.
pub async fn list_revisions(
    pool: &PgPool,
    circuit_id: &str,
) -> Result<Vec<RevisionSummary>, Error> {
    let rows: Vec<RevisionSummary> = sqlx::query_as(
        "SELECT r.number, r.created_by, \
                to_char(r.created_at, 'YYYY-MM-DD HH24:MI') AS created_at, r.message, \
                o.number AS restored_from \
         FROM circuit_revisions r LEFT JOIN circuit_revisions o ON o.id = r.restored_from \
         WHERE r.circuit_id = $1 ORDER BY r.number DESC",
    )
    .bind(circuit_uuid(circuit_id)?)
    .fetch_all(pool)
    .await?;
    // Every circuit has revision 1, so no rows means no such circuit.
    if rows.is_empty() {
        return Err(unknown_circuit());
    }
    Ok(rows)
}

/// Save a copy of revision `number` as a new revision on top of `base_revision`, with the
/// message "Restored revision N". Returns the new revision's number, or
/// [`Error::Conflict`] if someone saved meanwhile.
pub async fn restore_revision(
    pool: &PgPool,
    circuit_id: &str,
    number: i32,
    base_revision: i32,
) -> Result<i32, Error> {
    let mut tx = pool.begin().await?;
    let (id, next) = lock_base(&mut tx, circuit_id, base_revision).await?;
    let (revision, name): (Uuid, String) = sqlx::query_as(
        "INSERT INTO circuit_revisions (circuit_id, number, document, message, restored_from) \
         SELECT circuit_id, $3, document, $4, id FROM circuit_revisions \
         WHERE circuit_id = $1 AND number = $2 \
         RETURNING id, document->>'name'",
    )
    .bind(id)
    .bind(number)
    .bind(next)
    .bind(format!("Restored revision {number}"))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::Invalid(vec![format!("unknown revision {number}")]))?;
    set_current(&mut tx, id, revision, &name).await?;
    tx.commit().await?;
    Ok(next)
}
