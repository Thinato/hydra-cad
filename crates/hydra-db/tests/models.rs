use std::path::Path;

use hydra_core::{Catalogue, CatalogueRef};
use hydra_db::{
    Error, ModelOwner, asset_bytes, attach_model, import_catalogue, load_catalogue,
    save_valve_type, sha256_hex,
};
use sqlx::PgPool;

const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

fn repo_file(path: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .unwrap()
}

fn fixture() -> Catalogue {
    serde_json::from_slice(&repo_file("fixtures/catalogue.json")).unwrap()
}

fn valve_glb() -> Vec<u8> {
    repo_file("fixtures/models/demo-valve-cetop3.glb")
}

fn block_glb() -> Vec<u8> {
    repo_file("fixtures/models/demo-block.glb")
}

fn invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> Vec<String> {
    match r {
        Err(Error::Invalid(msgs)) => msgs,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// The fixture's demo block and the RPE3 valve type, both version 1.
fn owners() -> (ModelOwner, ModelOwner) {
    (
        ModelOwner::Block(CatalogueRef::new("blk-demo", 1)),
        ModelOwner::ValveType(CatalogueRef::new("vt-rpe3-06-z11", 1)),
    )
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn blocks_import_and_load(pool: PgPool) {
    let f = fixture();
    assert_eq!(import_catalogue(&pool, &f).await.unwrap().blocks_added, 1);
    let loaded = load_catalogue(&pool).await.unwrap().catalogue;
    assert_eq!(loaded.blocks, f.blocks);
    assert_eq!(loaded.blocks[0].mount_faces.len(), 3);

    let again = import_catalogue(&pool, &f).await.unwrap();
    assert_eq!(again.blocks_added, 0);

    let mut changed = f.clone();
    changed.blocks[0].mount_faces[2].origin[0] = 31.0;
    assert_eq!(
        invalid(import_catalogue(&pool, &changed).await),
        ["block blk-demo@1 already exists with different content; import it as version 2"]
    );
    changed.blocks[0].version = 2;
    assert_eq!(
        import_catalogue(&pool, &changed)
            .await
            .unwrap()
            .blocks_added,
        1
    );
    let blocks = load_catalogue(&pool).await.unwrap().catalogue.blocks;
    assert_eq!(blocks, [f.blocks[0].clone(), changed.blocks[0].clone()]);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn attached_models_load_and_replace(pool: PgPool) {
    import_catalogue(&pool, &fixture()).await.unwrap();
    let (block, valve) = owners();
    let (vglb, bglb) = (valve_glb(), block_glb());
    let block_sha = attach_model(&pool, &block, &bglb, Some(STEP))
        .await
        .unwrap();
    let valve_sha = attach_model(&pool, &valve, &vglb, None).await.unwrap();
    assert_eq!(block_sha, sha256_hex(&bglb));
    assert_eq!(valve_sha, sha256_hex(&vglb));

    let loaded = load_catalogue(&pool).await.unwrap();
    let (ModelOwner::Block(b), ModelOwner::ValveType(v)) = (&block, &valve) else {
        unreachable!()
    };
    assert_eq!(loaded.block_models.get(b), Some(&block_sha));
    assert_eq!(loaded.valve_models.get(v), Some(&valve_sha));
    assert_eq!(loaded.valve_models.len() + loaded.block_models.len(), 2);
    assert_eq!(
        asset_bytes(&pool, &block_sha).await.unwrap().as_deref(),
        Some(&bglb[..])
    );
    let source = "SELECT count(*) FROM part_assets WHERE role = 'source'";
    assert_eq!(count(&pool, source).await, 1);

    // A new model without a source replaces the display model and drops the old source.
    let new_sha = attach_model(&pool, &block, &vglb, None).await.unwrap();
    assert_eq!(new_sha, valve_sha);
    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.block_models.get(b), Some(&valve_sha));
    assert_eq!(count(&pool, source).await, 0);
    // The glb bytes are stored once, shared by both owners; old assets stay.
    assert_eq!(count(&pool, "SELECT count(*) FROM assets").await, 3);
    assert_eq!(count(&pool, "SELECT count(*) FROM part_assets").await, 2);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn bad_models_and_owners_are_refused(pool: PgPool) {
    import_catalogue(&pool, &fixture()).await.unwrap();
    let (block, _) = owners();
    let glb = block_glb();

    let msgs = invalid(attach_model(&pool, &block, b"glTF nonsense", None).await);
    assert_eq!(msgs.len(), 1);
    assert!(
        msgs[0].starts_with("the 3D model can't be used: "),
        "{msgs:?}"
    );
    assert_eq!(
        invalid(attach_model(&pool, &block, &glb, Some(b"%PDF-1.4")).await),
        ["the source file is not a STEP file"]
    );
    assert_eq!(
        invalid(
            attach_model(
                &pool,
                &ModelOwner::Block(CatalogueRef::new("blk-missing", 1)),
                &glb,
                None
            )
            .await
        ),
        ["unknown block blk-missing@1"]
    );
    assert_eq!(
        invalid(
            attach_model(
                &pool,
                &ModelOwner::ValveType(CatalogueRef::new("vt-missing", 1)),
                &glb,
                None
            )
            .await
        ),
        ["unknown valve type vt-missing@1"]
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM assets").await, 0);

    // A byte-order mark and leading whitespace are fine before the STEP magic.
    let step = [b"\xEF\xBB\xBF \r\n".as_slice(), STEP].concat();
    attach_model(&pool, &block, &glb, Some(&step))
        .await
        .unwrap();
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_carries_models_forward(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let (_, valve) = owners();
    let sha = attach_model(&pool, &valve, &valve_glb(), Some(STEP))
        .await
        .unwrap();
    let ModelOwner::ValveType(v1) = valve else {
        unreachable!()
    };
    let mut edited = f.valve_type(&v1).unwrap().clone();
    edited.code = "Z11-B".into();
    let v2 = save_valve_type(&pool, &edited).await.unwrap();

    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.valve_models.get(&v1), Some(&sha));
    assert_eq!(loaded.valve_models.get(&v2), Some(&sha));
    let source = "SELECT count(*) FROM part_assets WHERE role = 'source'";
    assert_eq!(count(&pool, source).await, 2);

    // An imported later version inherits them too, like the datasheet.
    let mut v3 = edited.clone();
    v3.version = 3;
    let more = Catalogue {
        valve_types: vec![v3.clone()],
        ..f
    };
    import_catalogue(&pool, &more).await.unwrap();
    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.valve_models.get(&v3.reference()), Some(&sha));

    // So does a block's next version.
    let (block, _) = owners();
    let block_sha = attach_model(&pool, &block, &block_glb(), None)
        .await
        .unwrap();
    let mut b2 = more.blocks[0].clone();
    b2.version = 2;
    b2.name = "Demo block, faces corrected".into();
    let blocks = Catalogue {
        blocks: vec![b2.clone()],
        ..more
    };
    import_catalogue(&pool, &blocks).await.unwrap();
    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.block_models.get(&b2.reference()), Some(&block_sha));
}

/// Attaching to key@N while an import adds key@N+1 must not deadlock.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn attach_and_import_run_together(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let base = f
        .valve_type(&CatalogueRef::new("vt-rpe3-06-z11", 1))
        .unwrap()
        .clone();
    let glb = valve_glb();
    for n in 1..=10 {
        let owner = ModelOwner::ValveType(CatalogueRef::new(&base.key, n));
        let mut next = base.clone();
        next.version = n + 1;
        let incoming = Catalogue {
            valve_types: vec![next],
            ..f.clone()
        };
        let (attached, imported) = tokio::join!(
            attach_model(&pool, &owner, &glb, None),
            import_catalogue(&pool, &incoming)
        );
        // A deadlock would fail one of them.
        attached.unwrap();
        imported.unwrap();
    }
}
