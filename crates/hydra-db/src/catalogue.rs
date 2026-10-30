//! Catalogue repository: load, import and save versioned catalogue rows, and their datasheet
//! and 3D model assets.
//!
//! Rows are found by `key` (and `version`); uuids never leave this module.

use std::collections::HashMap;

use hydra_core::{
    Block, Catalogue, CatalogueRef, Manufacturer, MountFace, Part, SymbolDef, SymbolDefRecord,
    ValveType, key_problem,
};
use serde_json::Value;
use sqlx::{PgConnection, PgPool, types::Json};

use crate::{Error, sha256_hex};

/// The whole catalogue, plus the datasheet and display model hashes of the rows that have one.
#[derive(Debug, Clone, Default)]
pub struct LoadedCatalogue {
    pub catalogue: Catalogue,
    /// sha256 hex of each valve type's datasheet
    pub datasheets: HashMap<CatalogueRef, String>,
    /// sha256 hex of each valve type's display model (glb)
    pub valve_models: HashMap<CatalogueRef, String>,
    /// sha256 hex of each block's display model (glb)
    pub block_models: HashMap<CatalogueRef, String>,
}

/// The catalogue row a 3D model belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelOwner {
    ValveType(CatalogueRef),
    Block(CatalogueRef),
}

/// What [`import_catalogue`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub manufacturers_added: usize,
    pub symbol_defs_added: usize,
    pub valve_types_added: usize,
    pub parts_added: usize,
    pub blocks_added: usize,
    /// Rows already present with identical content.
    pub unchanged: usize,
}

/// Every catalogue row, every version, ordered by key then version; parts by valve type
/// then part number. Datasheet and model hashes too. All read from one snapshot.
pub async fn load_catalogue(pool: &PgPool) -> Result<LoadedCatalogue, Error> {
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    let loaded = load(&mut tx).await?;
    tx.commit().await?;
    Ok(loaded)
}

/// A version as Postgres stores it: 1 to `i32::MAX`.
pub(crate) fn db_version(version: u32) -> Result<i32, Error> {
    i32::try_from(version)
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::Invalid(vec![format!("version {version} is out of range")]))
}

/// Turn a unique violation (SQLSTATE 23505) into `Invalid(msg)`; other errors pass through.
/// Covers rows another session added between our check and our insert.
fn unique_violation<E: Into<Error>>(msg: impl FnOnce() -> String) -> impl FnOnce(E) -> Error {
    move |e| match e.into() {
        Error::Sqlx(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_unique_violation()) =>
        {
            Error::Invalid(vec![msg()])
        }
        e => e,
    }
}

/// SQL NULL for `null`, so it reads back as `Value::Null`.
fn json_or_null(v: &Value) -> Option<Json<&Value>> {
    (!v.is_null()).then_some(Json(v))
}

// Keys are ordered with the "C" collation so the order matches Rust's string order.
pub(crate) async fn load(conn: &mut PgConnection) -> Result<LoadedCatalogue, Error> {
    let manufacturers: Vec<(String, String)> =
        sqlx::query_as("SELECT key, name FROM manufacturers ORDER BY key COLLATE \"C\"")
            .fetch_all(&mut *conn)
            .await?;
    let symbol_defs: Vec<(String, i32, Json<SymbolDef>)> = sqlx::query_as(
        "SELECT key, version, definition FROM symbol_defs ORDER BY key COLLATE \"C\", version",
    )
    .fetch_all(&mut *conn)
    .await?;
    type ValveRow = (
        String,
        i32,
        String,
        String,
        String,
        Option<String>,
        Option<i32>,
        Option<String>,
        Option<Value>,
        Option<String>,
    );
    let valve_types: Vec<ValveRow> = sqlx::query_as(
        "SELECT v.key, v.version, m.key, v.series, v.code, s.key, s.version, v.mounting, \
                v.params, a.sha256 \
         FROM valve_types v \
         JOIN manufacturers m ON m.id = v.manufacturer_id \
         LEFT JOIN symbol_defs s ON s.id = v.symbol_def_id \
         LEFT JOIN assets a ON a.id = v.datasheet_asset_id \
         ORDER BY v.key COLLATE \"C\", v.version",
    )
    .fetch_all(&mut *conn)
    .await?;
    let parts: Vec<(String, String, i32, Option<Value>)> = sqlx::query_as(
        "SELECT p.part_number, v.key, v.version, p.options \
         FROM parts p JOIN valve_types v ON v.id = p.valve_type_id \
         ORDER BY v.key COLLATE \"C\", v.version, p.part_number COLLATE \"C\"",
    )
    .fetch_all(&mut *conn)
    .await?;
    let blocks: Vec<(String, i32, String, Json<Vec<MountFace>>)> = sqlx::query_as(
        "SELECT key, version, name, mount_faces FROM blocks ORDER BY key COLLATE \"C\", version",
    )
    .fetch_all(&mut *conn)
    .await?;
    let models: Vec<(bool, String, i32, String)> = sqlx::query_as(
        "SELECT false, v.key, v.version, a.sha256 \
         FROM part_assets pa JOIN valve_types v ON v.id = pa.owner_id \
         JOIN assets a ON a.id = pa.asset_id WHERE pa.role = 'display' \
         UNION ALL \
         SELECT true, b.key, b.version, a.sha256 \
         FROM part_assets pa JOIN blocks b ON b.id = pa.owner_id \
         JOIN assets a ON a.id = pa.asset_id WHERE pa.role = 'display'",
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut loaded = LoadedCatalogue::default();
    for (is_block, key, version, sha) in models {
        let r = CatalogueRef::new(key, version as u32);
        if is_block {
            loaded.block_models.insert(r, sha);
        } else {
            loaded.valve_models.insert(r, sha);
        }
    }
    let c = &mut loaded.catalogue;
    c.manufacturers = manufacturers
        .into_iter()
        .map(|(key, name)| Manufacturer { key, name })
        .collect();
    c.symbol_defs = symbol_defs
        .into_iter()
        .map(|(key, version, Json(definition))| SymbolDefRecord {
            key,
            version: version as u32,
            definition,
        })
        .collect();
    for (key, version, manufacturer, series, code, sym_key, sym_version, mounting, params, sha) in
        valve_types
    {
        let v = ValveType {
            key,
            version: version as u32,
            manufacturer,
            series,
            code,
            symbol: sym_key
                .zip(sym_version)
                .map(|(k, n)| CatalogueRef::new(k, n as u32)),
            mounting,
            params: params.unwrap_or_default(),
        };
        if let Some(sha) = sha {
            loaded.datasheets.insert(v.reference(), sha);
        }
        c.valve_types.push(v);
    }
    c.parts = parts
        .into_iter()
        .map(|(part_number, key, version, options)| Part {
            part_number,
            valve_type: CatalogueRef::new(key, version as u32),
            options: options.unwrap_or_default(),
        })
        .collect();
    c.blocks = blocks
        .into_iter()
        .map(|(key, version, name, Json(mount_faces))| Block {
            key,
            version: version as u32,
            name,
            mount_faces,
        })
        .collect();
    Ok(loaded)
}

/// Insert one valve type row at `version`, resolving manufacturer and symbol by key.
/// The row inherits the datasheet and 3D models of the latest lower version of its key, if any.
pub(crate) async fn insert_valve_type(
    conn: &mut PgConnection,
    v: &ValveType,
    version: i32,
) -> Result<(), Error> {
    let symbol_version = v
        .symbol
        .as_ref()
        .map(|s| db_version(s.version))
        .transpose()?;
    // Wait for a datasheet or model being attached to the version this one inherits from,
    // so the copy below sees it.
    sqlx::query(
        "SELECT id FROM valve_types WHERE key = $1 AND version < $2 \
         ORDER BY version DESC LIMIT 1 FOR SHARE",
    )
    .bind(&v.key)
    .bind(version)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO valve_types \
             (manufacturer_id, series, code, symbol_def_id, mounting, params, key, version, \
              datasheet_asset_id) \
         VALUES ((SELECT id FROM manufacturers WHERE key = $1), $2, $3, \
                 (SELECT id FROM symbol_defs WHERE key = $4 AND version = $5), $6, $7, $8, $9, \
                 (SELECT datasheet_asset_id FROM valve_types WHERE key = $8 AND version < $9 \
                  ORDER BY version DESC LIMIT 1))",
    )
    .bind(&v.manufacturer)
    .bind(&v.series)
    .bind(&v.code)
    .bind(v.symbol.as_ref().map(|s| &s.key))
    .bind(symbol_version)
    .bind(&v.mounting)
    .bind(json_or_null(&v.params))
    .bind(&v.key)
    .bind(version)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO part_assets (owner_id, asset_id, role) \
         SELECT (SELECT id FROM valve_types WHERE key = $1 AND version = $2), pa.asset_id, pa.role \
         FROM part_assets pa \
         WHERE pa.owner_id = (SELECT id FROM valve_types WHERE key = $1 AND version < $2 \
                              ORDER BY version DESC LIMIT 1)",
    )
    .bind(&v.key)
    .bind(version)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writers of 3D models (attach, save and import, which copy them to new versions) take this
/// lock first, before any row lock, so they wait for each other in one order and never
/// deadlock, and a new version always copies the models committed last.
async fn lock_models(conn: &mut PgConnection) -> Result<(), Error> {
    sqlx::query("LOCK TABLE part_assets IN SHARE ROW EXCLUSIVE MODE")
        .execute(conn)
        .await?;
    Ok(())
}

/// Lock every catalogue table against other writers, `part_assets` first (see `lock_models`).
/// Validate-then-insert needs every committed row in view and no new ones until we commit.
/// SHARE ROW EXCLUSIVE conflicts with itself and with writers, so concurrent imports, saves,
/// approvals and attaches wait for us while readers carry on. Take it before the first read
/// of a REPEATABLE READ transaction, so the snapshot includes everything committed before the
/// lock. Chosen over SERIALIZABLE because it needs no retry loop.
pub(crate) async fn lock_catalogue(conn: &mut PgConnection) -> Result<(), Error> {
    sqlx::query(
        "LOCK TABLE part_assets, manufacturers, symbol_defs, valve_types, parts, blocks \
         IN SHARE ROW EXCLUSIVE MODE",
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn insert_symbol_def(
    conn: &mut PgConnection,
    s: &SymbolDefRecord,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO symbol_defs (kind, definition, key, version) \
         VALUES ($1->>'kind', $1, $2, $3)",
    )
    .bind(Json(&s.definition))
    .bind(&s.key)
    .bind(db_version(s.version)?)
    .execute(conn)
    .await
    .map_err(unique_violation(|| {
        format!("symbol {} already exists", s.reference())
    }))?;
    Ok(())
}

pub(crate) async fn insert_part(conn: &mut PgConnection, p: &Part) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO parts (valve_type_id, part_number, options) \
         VALUES ((SELECT id FROM valve_types WHERE key = $1 AND version = $2), $3, $4)",
    )
    .bind(&p.valve_type.key)
    .bind(db_version(p.valve_type.version)?)
    .bind(&p.part_number)
    .bind(json_or_null(&p.options))
    .execute(conn)
    .await
    .map_err(unique_violation(|| {
        format!("part {} of {} already exists", p.part_number, p.valve_type)
    }))?;
    Ok(())
}

/// Add the rows of `incoming` that are new, in one transaction.
///
/// Rows identical to existing ones are skipped. A key@version or part that exists with
/// different content, a renamed manufacturer, or any [`Catalogue::problems`] of the merged
/// catalogue fails the whole import with [`Error::Invalid`].
pub async fn import_catalogue(pool: &PgPool, incoming: &Catalogue) -> Result<ImportReport, Error> {
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await?;
    lock_catalogue(&mut tx).await?;
    let mut merged = load(&mut tx).await?.catalogue;
    // Rows already stored stay importable alongside, even if a later build checks harder.
    let before = merged.problems();
    let mut report = ImportReport::default();
    let mut errors = Vec::new();
    let (mut new_m, mut new_s, mut new_v, mut new_p, mut new_b) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());

    for m in &incoming.manufacturers {
        match merged.manufacturers.iter().find(|e| e.key == m.key) {
            Some(e) if e.name == m.name => report.unchanged += 1,
            Some(e) => errors.push(format!(
                "manufacturer {} already exists with name {:?}, not {:?}",
                m.key, e.name, m.name
            )),
            None => new_m.push(m),
        }
    }
    for s in &incoming.symbol_defs {
        let r = s.reference();
        match merged.symbol_defs.iter().find(|e| e.reference() == r) {
            Some(e) if e == s => report.unchanged += 1,
            Some(_) => errors.push(format!(
                "symbol {r} already exists with different content; import it as version {}",
                r.version + 1
            )),
            None => new_s.push(s),
        }
    }
    for v in &incoming.valve_types {
        let r = v.reference();
        match merged.valve_types.iter().find(|e| e.reference() == r) {
            Some(e) if e == v => report.unchanged += 1,
            Some(_) => errors.push(format!(
                "valve type {r} already exists with different content; import it as version {}",
                r.version + 1
            )),
            None => new_v.push(v),
        }
    }
    for p in &incoming.parts {
        let existing = merged
            .parts
            .iter()
            .find(|e| e.valve_type == p.valve_type && e.part_number == p.part_number);
        match existing {
            Some(e) if e == p => report.unchanged += 1,
            Some(_) => errors.push(format!(
                "part {} of {} already exists with different options",
                p.part_number, p.valve_type
            )),
            None => new_p.push(p),
        }
    }
    for b in &incoming.blocks {
        let r = b.reference();
        match merged.blocks.iter().find(|e| e.reference() == r) {
            Some(e) if e == b => report.unchanged += 1,
            Some(_) => errors.push(format!(
                "block {r} already exists with different content; import it as version {}",
                r.version + 1
            )),
            None => new_b.push(b),
        }
    }

    merged
        .manufacturers
        .extend(new_m.iter().map(|&m| m.clone()));
    merged.symbol_defs.extend(new_s.iter().map(|&s| s.clone()));
    merged.valve_types.extend(new_v.iter().map(|&v| v.clone()));
    merged.parts.extend(new_p.iter().map(|&p| p.clone()));
    merged.blocks.extend(new_b.iter().map(|&b| b.clone()));
    errors.extend(
        merged
            .problems()
            .into_iter()
            .filter(|p| !before.contains(p)),
    );
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }

    for m in &new_m {
        sqlx::query("INSERT INTO manufacturers (key, name) VALUES ($1, $2)")
            .bind(&m.key)
            .bind(&m.name)
            .execute(&mut *tx)
            .await
            .map_err(unique_violation(|| {
                format!("manufacturer name {:?} is already used", m.name)
            }))?;
    }
    for s in &new_s {
        insert_symbol_def(&mut tx, s).await?;
    }
    for v in &new_v {
        insert_valve_type(&mut tx, v, db_version(v.version)?)
            .await
            .map_err(unique_violation(|| {
                format!("valve type {} already exists", v.reference())
            }))?;
    }
    for p in &new_p {
        insert_part(&mut tx, p).await?;
    }
    for b in &new_b {
        sqlx::query("INSERT INTO blocks (name, mount_faces, key, version) VALUES ($1, $2, $3, $4)")
            .bind(&b.name)
            .bind(Json(&b.mount_faces))
            .bind(&b.key)
            .bind(db_version(b.version)?)
            .execute(&mut *tx)
            .await
            .map_err(unique_violation(|| {
                format!("block {} already exists", b.reference())
            }))?;
        // Like a valve type, a new version shows the previous version's 3D model until
        // it gets its own.
        sqlx::query(
            "INSERT INTO part_assets (owner_id, asset_id, role) \
             SELECT (SELECT id FROM blocks WHERE key = $1 AND version = $2), pa.asset_id, pa.role \
             FROM part_assets pa \
             WHERE pa.owner_id = (SELECT id FROM blocks WHERE key = $1 AND version < $2 \
                                  ORDER BY version DESC LIMIT 1)",
        )
        .bind(&b.key)
        .bind(db_version(b.version)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    report.manufacturers_added = new_m.len();
    report.symbol_defs_added = new_s.len();
    report.valve_types_added = new_v.len();
    report.parts_added = new_p.len();
    report.blocks_added = new_b.len();
    Ok(report)
}

/// Save an edited valve type as a new version of its key.
///
/// `valve_type.version` is the version the edit started from, or 0 for a new key. The save is
/// refused with [`Error::Invalid`] if the key has moved past that version (someone else saved
/// meanwhile) or, for a new key, if the key already exists. Otherwise it inserts
/// `key@(version + 1)`, which inherits the base version's datasheet, parts and 3D models.
pub async fn save_valve_type(pool: &PgPool, valve_type: &ValveType) -> Result<CatalogueRef, Error> {
    let key = &valve_type.key;
    if let Some(p) = key_problem(key) {
        return Err(Error::Invalid(vec![format!("valve type key {key:?} {p}")]));
    }
    let base = valve_type.version;
    let next = db_version(base.saturating_add(1))?;
    let changed_meanwhile =
        || format!("valve type {key} was changed meanwhile; reload and edit again");
    let mut v = valve_type.clone();
    v.series = v.series.trim().into();
    v.code = v.code.trim().into();

    let mut tx = pool.begin().await?;
    lock_models(&mut tx).await?;
    let latest: i32 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM valve_types WHERE key = $1")
            .bind(key)
            .fetch_one(&mut *tx)
            .await?;
    if latest != next - 1 {
        return Err(Error::Invalid(vec![if base == 0 {
            format!("valve type key {key} already exists")
        } else {
            format!(
                "valve type {key} was changed to version {latest} meanwhile; reload and edit again"
            )
        }]));
    }
    let mut errors = Vec::new();
    let has_manufacturer: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM manufacturers WHERE key = $1)")
            .bind(&v.manufacturer)
            .fetch_one(&mut *tx)
            .await?;
    if !has_manufacturer {
        errors.push(format!("unknown manufacturer {}", v.manufacturer));
    }
    if let Some(s) = &v.symbol {
        let has_symbol: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM symbol_defs WHERE key = $1 AND version = $2)",
        )
        .bind(&s.key)
        .bind(db_version(s.version)?)
        .fetch_one(&mut *tx)
        .await?;
        if !has_symbol {
            errors.push(format!("unknown symbol {s}"));
        }
    }
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    // Two saves from the same base can both pass the check above; the second insert then
    // hits UNIQUE (key, version).
    insert_valve_type(&mut tx, &v, next)
        .await
        .map_err(unique_violation(changed_meanwhile))?;
    sqlx::query(
        "INSERT INTO parts (valve_type_id, part_number, options) \
         SELECT (SELECT id FROM valve_types WHERE key = $1 AND version = $3), \
                p.part_number, p.options \
         FROM parts p JOIN valve_types o ON o.id = p.valve_type_id \
         WHERE o.key = $1 AND o.version = $2",
    )
    .bind(key)
    .bind(next - 1)
    .bind(next)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(CatalogueRef::new(key, base + 1))
}

/// Store a PDF datasheet (once per content hash) and link it to one valve type row.
/// Returns the sha256 hex of the bytes.
pub async fn attach_datasheet(
    pool: &PgPool,
    valve_type: &CatalogueRef,
    pdf: &[u8],
) -> Result<String, Error> {
    if !pdf.starts_with(b"%PDF-") {
        return Err(Error::Invalid(vec!["not a PDF file".into()]));
    }
    let version = db_version(valve_type.version)?;
    let sha = sha256_hex(pdf);
    let mut tx = pool.begin().await?;
    // ON CONFLICT also covers a concurrent attach of the same bytes, so no unique violation.
    sqlx::query(
        "INSERT INTO assets (sha256, kind, bytes, size) VALUES ($1, 'pdf', $2, $3) \
         ON CONFLICT (sha256) DO NOTHING",
    )
    .bind(&sha)
    .bind(pdf)
    .bind(i64::try_from(pdf.len()).unwrap_or(i64::MAX))
    .execute(&mut *tx)
    .await?;
    // The one in-place update of a catalogue row (the database allows only this one): a
    // datasheet does not change how any circuit draws, so it needs no new version.
    let updated = sqlx::query(
        "UPDATE valve_types SET datasheet_asset_id = (SELECT id FROM assets WHERE sha256 = $1) \
         WHERE key = $2 AND version = $3",
    )
    .bind(&sha)
    .bind(&valve_type.key)
    .bind(version)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(Error::Invalid(vec![format!(
            "unknown valve type {valve_type}"
        )]));
    }
    tx.commit().await?;
    Ok(sha)
}

/// Store a display model (glb) and optional STEP source (once per content hash each) and make
/// them the owner's models, replacing any it had. Returns the sha256 hex of the glb.
pub async fn attach_model(
    pool: &PgPool,
    owner: &ModelOwner,
    glb: &[u8],
    step: Option<&[u8]>,
) -> Result<String, Error> {
    if let Err(e) = hydra_mesh::load_glb(glb) {
        return Err(Error::Invalid(vec![format!(
            "the 3D model can't be used: {e}"
        )]));
    }
    if let Some(step) = step {
        let text = step.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(step);
        if !text.trim_ascii_start().starts_with(b"ISO-10303-21;") {
            return Err(Error::Invalid(vec![
                "the source file is not a STEP file".into(),
            ]));
        }
    }
    let (sql, what, r) = match owner {
        ModelOwner::ValveType(r) => (
            "SELECT id FROM valve_types WHERE key = $1 AND version = $2 FOR UPDATE",
            "valve type",
            r,
        ),
        ModelOwner::Block(r) => (
            "SELECT id FROM blocks WHERE key = $1 AND version = $2 FOR UPDATE",
            "block",
            r,
        ),
    };
    let version = db_version(r.version)?;
    let mut tx = pool.begin().await?;
    lock_models(&mut tx).await?;
    let owner_id: Option<sqlx::types::Uuid> = sqlx::query_scalar(sql)
        .bind(&r.key)
        .bind(version)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(owner_id) = owner_id else {
        return Err(Error::Invalid(vec![format!("unknown {what} {r}")]));
    };
    sqlx::query("DELETE FROM part_assets WHERE owner_id = $1")
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;
    let glb_sha = sha256_hex(glb);
    let assets = [
        Some(("gltf", "display", glb)),
        step.map(|s| ("step", "source", s)),
    ];
    for (kind, role, bytes) in assets.into_iter().flatten() {
        let sha = sha256_hex(bytes);
        sqlx::query(
            "INSERT INTO assets (sha256, kind, bytes, size) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (sha256) DO NOTHING",
        )
        .bind(&sha)
        .bind(kind)
        .bind(bytes)
        .bind(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO part_assets (owner_id, asset_id, role) \
             VALUES ($1, (SELECT id FROM assets WHERE sha256 = $2), $3)",
        )
        .bind(owner_id)
        .bind(&sha)
        .bind(role)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(glb_sha)
}

/// The bytes of a stored asset, by sha256 hex.
pub async fn asset_bytes(pool: &PgPool, sha256: &str) -> Result<Option<Vec<u8>>, Error> {
    Ok(
        sqlx::query_scalar("SELECT bytes FROM assets WHERE sha256 = $1")
            .bind(sha256)
            .fetch_optional(pool)
            .await?,
    )
}
