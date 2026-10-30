//! Review queue for catalogue proposals: an assistant submits a [`Proposal`], an engineer
//! approves it (possibly edited), which saves it as catalogue rows, or rejects it with a note.
//!
//! Proposal ids are uuids passed as text. Reviewed proposals are kept as history.

use hydra_core::{CatalogueRef, Proposal, ValveType};
use sqlx::{PgConnection, PgPool, types::Json, types::Uuid};

use crate::Error;

/// Largest proposal JSON accepted, in bytes.
pub const MAX_PROPOSAL_BYTES: usize = 256 * 1024;
use crate::catalogue::{
    db_version, insert_part, insert_symbol_def, insert_valve_type, load, load_catalogue,
    lock_catalogue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
}

/// One proposal in the review queue.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRecord {
    /// The proposal's uuid as text.
    pub id: String,
    pub status: ProposalStatus,
    /// As submitted.
    pub proposal: Proposal,
    /// As approved, which may differ from `proposal` by the reviewer's edits.
    pub approved: Option<Proposal>,
    /// sha256 hex of the datasheet the valve type had when the proposal was submitted.
    pub datasheet_sha256: Option<String>,
    /// Postgres role that submitted it.
    pub created_by: String,
    /// `YYYY-MM-DD HH24:MI` in the session time zone.
    pub created_at: String,
    /// Postgres role that approved or rejected it.
    pub reviewed_by: Option<String>,
    /// `YYYY-MM-DD HH24:MI` in the session time zone.
    pub reviewed_at: Option<String>,
    /// Why it was rejected.
    pub review_note: Option<String>,
}

fn unknown_proposal() -> Error {
    Error::Invalid(vec!["unknown proposal".into()])
}

/// Bound as a uuid, so a malformed id is refused here and never reaches the SQL text.
fn proposal_uuid(id: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(id).map_err(|_| unknown_proposal())
}

/// Queue `proposal` for review. Refused with [`Error::Invalid`] if it has
/// [`Proposal::problems`] against the catalogue as it is now. Returns the proposal's id.
pub async fn submit_proposal(pool: &PgPool, proposal: &Proposal) -> Result<String, Error> {
    // Bounds what reviewers load and what every check walks; a datasheet needs far less.
    let size = serde_json::to_vec(proposal).map_or(0, |j| j.len());
    if size > MAX_PROPOSAL_BYTES {
        return Err(Error::Invalid(vec![format!(
            "the proposal is {} KiB; at most {} KiB fits a valve type",
            size / 1024,
            MAX_PROPOSAL_BYTES / 1024
        )]));
    }
    // No locks: approval checks the proposal again against the catalogue as it is then.
    let problems = proposal.problems(&load_catalogue(pool).await?.catalogue);
    if !problems.is_empty() {
        return Err(Error::Invalid(problems));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalogue_proposals (proposal, datasheet_asset_id) \
         VALUES ($1, (SELECT datasheet_asset_id FROM valve_types WHERE key = $2 \
                          ORDER BY version DESC LIMIT 1)) \
         RETURNING id",
    )
    .bind(Json(proposal))
    .bind(&proposal.valve_type.key)
    .fetch_one(pool)
    .await?;
    Ok(id.to_string())
}

type ProposalRow = (
    String,
    String,
    Json<serde_json::Value>,
    Option<Json<serde_json::Value>>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// At most 200 proposals: the one with `id` if given; pending ones only, oldest first (the
/// queue), if `pending_only`; otherwise newest first. Rows this build can't read are skipped.
async fn fetch(
    pool: &PgPool,
    id: Option<Uuid>,
    pending_only: bool,
) -> Result<Vec<ProposalRecord>, Error> {
    let rows: Vec<ProposalRow> = sqlx::query_as(
        "SELECT p.id::text, p.status, p.proposal, p.approved, a.sha256, p.created_by, \
                to_char(p.created_at, 'YYYY-MM-DD HH24:MI'), p.reviewed_by, \
                to_char(p.reviewed_at, 'YYYY-MM-DD HH24:MI'), p.review_note \
         FROM catalogue_proposals p LEFT JOIN assets a ON a.id = p.datasheet_asset_id \
         WHERE ($1::uuid IS NULL OR p.id = $1) AND (NOT $2 OR p.status = 'pending') \
         ORDER BY CASE WHEN $2 THEN p.created_at END ASC, p.created_at DESC, p.id \
         LIMIT 200",
    )
    .bind(id)
    .bind(pending_only)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(
                id,
                status,
                Json(proposal),
                approved,
                datasheet_sha256,
                created_by,
                created_at,
                reviewed_by,
                reviewed_at,
                review_note,
            )| {
                // One unreadable row (say, written by a newer hydra-mcp) must not hide the queue.
                let read = |v: serde_json::Value| serde_json::from_value::<Proposal>(v);
                let (proposal, approved) = match (read(proposal), approved.map(|Json(a)| read(a))) {
                    (Ok(p), None) => (p, None),
                    (Ok(p), Some(Ok(a))) => (p, Some(a)),
                    (Err(e), _) | (_, Some(Err(e))) => {
                        tracing::warn!("skipping proposal {id}: {e}");
                        return None;
                    }
                };
                Some(ProposalRecord {
                    id,
                    // The table's CHECK allows only these three.
                    status: match status.as_str() {
                        "approved" => ProposalStatus::Approved,
                        "rejected" => ProposalStatus::Rejected,
                        _ => ProposalStatus::Pending,
                    },
                    proposal,
                    approved,
                    datasheet_sha256,
                    created_by,
                    created_at,
                    reviewed_by,
                    reviewed_at,
                    review_note,
                })
            },
        )
        .collect())
}

/// At most 200 proposals: pending ones oldest first, or all of them newest first.
pub async fn list_proposals(
    pool: &PgPool,
    pending_only: bool,
) -> Result<Vec<ProposalRecord>, Error> {
    fetch(pool, None, pending_only).await
}

/// One proposal by id.
pub async fn get_proposal(pool: &PgPool, id: &str) -> Result<ProposalRecord, Error> {
    fetch(pool, Some(proposal_uuid(id)?), false)
        .await?
        .pop()
        .ok_or_else(unknown_proposal)
}

/// Lock the proposal row and check it is still pending. Returns the submitted proposal.
async fn lock_pending(conn: &mut PgConnection, id: Uuid) -> Result<Proposal, Error> {
    let row: Option<(String, Option<String>, Json<Proposal>)> = sqlx::query_as(
        "SELECT status, reviewed_by, proposal FROM catalogue_proposals WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.code()) {
        // Under REPEATABLE READ: a review committed after our snapshot was taken.
        Some(code) if code == "40001" => Error::Invalid(vec![
            "someone reviewed this proposal meanwhile; reload it".into(),
        ]),
        _ => e.into(),
    })?;
    let (status, reviewed_by, Json(proposal)) = row.ok_or_else(unknown_proposal)?;
    if status != "pending" {
        let who = reviewed_by.unwrap_or_default();
        return Err(Error::Invalid(vec![format!(
            "proposal was already {status} by {who}"
        )]));
    }
    Ok(proposal)
}

/// Approve proposal `id` as `edited` (the submitted proposal, or the reviewer's edit of it)
/// and save it to the catalogue, in one transaction: `edited.new_symbol` if any, then the
/// valve type as [`Proposal::target`] with the parts [`Proposal::apply`] gives it. The new
/// version inherits the datasheet and 3D models of the version it updates.
///
/// Refused with [`Error::Invalid`] if the proposal was already reviewed, the edit is for
/// another valve type key, or `edited` has [`Proposal::problems`] against the catalogue now,
/// for instance because someone saved the valve type since the proposal was submitted.
/// Returns the new valve type version.
pub async fn approve_proposal(
    pool: &PgPool,
    id: &str,
    edited: &Proposal,
) -> Result<CatalogueRef, Error> {
    let id = proposal_uuid(id)?;
    let target = edited.target();
    let version = db_version(target.version)?;
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await?;
    lock_catalogue(&mut tx).await?;
    let submitted = lock_pending(&mut tx, id).await?;
    if edited.valve_type.key != submitted.valve_type.key {
        return Err(Error::Invalid(vec![
            "a reviewer can't change which valve type a proposal is for".into(),
        ]));
    }
    let catalogue = load(&mut tx).await?.catalogue;
    let problems = edited.problems(&catalogue);
    if !problems.is_empty() {
        return Err(Error::Invalid(problems));
    }
    if let Some(s) = &edited.new_symbol {
        insert_symbol_def(&mut tx, s).await?;
    }
    let valve_type = ValveType {
        version: target.version,
        ..edited.valve_type.clone()
    };
    insert_valve_type(&mut tx, &valve_type, version).await?;
    for p in edited.apply(&catalogue).parts_of(&target) {
        insert_part(&mut tx, p).await?;
    }
    sqlx::query(
        "UPDATE catalogue_proposals \
         SET status = 'approved', reviewed_by = current_user, reviewed_at = now(), approved = $2 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(Json(edited))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(target)
}

/// Reject proposal `id`; `note` says why, for whoever drafted it.
pub async fn reject_proposal(pool: &PgPool, id: &str, note: &str) -> Result<(), Error> {
    let id = proposal_uuid(id)?;
    let note = note.trim();
    if note.is_empty() {
        return Err(Error::Invalid(vec![
            "say why, so the assistant can fix the proposal".into(),
        ]));
    }
    let mut tx = pool.begin().await?;
    lock_pending(&mut tx, id).await?;
    sqlx::query(
        "UPDATE catalogue_proposals \
         SET status = 'rejected', reviewed_by = current_user, reviewed_at = now(), \
             review_note = $2 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(note)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
