use std::path::Path;

use hydra_core::{Catalogue, CatalogueRef, Manufacturer, ValveType};
use hydra_db::{
    Error, asset_bytes, attach_datasheet, import_catalogue, load_catalogue, save_valve_type,
};
use sqlx::PgPool;

const PDF: &[u8] = b"%PDF-1.4 fake datasheet";

fn fixture() -> Catalogue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/catalogue.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Same order as `load_catalogue`: key, then version; parts by valve type, then part number.
fn sorted(mut c: Catalogue) -> Catalogue {
    c.manufacturers.sort_by(|a, b| a.key.cmp(&b.key));
    c.symbol_defs.sort_by_key(|s| s.reference());
    c.valve_types.sort_by_key(|v| v.reference());
    c.parts
        .sort_by(|a, b| (&a.valve_type, &a.part_number).cmp(&(&b.valve_type, &b.part_number)));
    c.blocks.sort_by_key(|b| b.reference());
    c
}

/// The fixture valve type with `key`.
fn valve(c: &Catalogue, key: &str) -> ValveType {
    c.valve_types.iter().find(|v| v.key == key).unwrap().clone()
}

fn invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> Vec<String> {
    match r {
        Err(Error::Invalid(msgs)) => msgs,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn import_then_load_round_trips(pool: PgPool) {
    let f = fixture();
    let r = import_catalogue(&pool, &f).await.unwrap();
    assert_eq!(r.manufacturers_added, f.manufacturers.len());
    assert_eq!(r.symbol_defs_added, f.symbol_defs.len());
    assert_eq!(r.valve_types_added, f.valve_types.len());
    assert_eq!(r.parts_added, f.parts.len());
    assert_eq!(r.blocks_added, f.blocks.len());
    assert!(!f.parts.is_empty());
    assert!(!f.blocks.is_empty());
    assert_eq!(r.unchanged, 0);
    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.catalogue, sorted(f));
    assert!(loaded.datasheets.is_empty());
    // `null` params and options are stored as SQL NULL.
    let nulls: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM valve_types WHERE params IS NULL) \
              + (SELECT count(*) FROM parts WHERE options IS NULL)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nulls, 8);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn reimport_is_all_unchanged(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let r = import_catalogue(&pool, &f).await.unwrap();
    let total = f.manufacturers.len()
        + f.symbol_defs.len()
        + f.valve_types.len()
        + f.parts.len()
        + f.blocks.len();
    assert_eq!(
        (
            r.manufacturers_added,
            r.symbol_defs_added,
            r.valve_types_added,
            r.parts_added,
            r.blocks_added
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(r.unchanged, total);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn conflicting_import_fails_and_writes_nothing(pool: PgPool) {
    import_catalogue(&pool, &fixture()).await.unwrap();
    let mut changed = fixture();
    changed.valve_types[0].code = "Z99".into();
    changed.parts[0].options = serde_json::json!({"voltage": "12 V DC"});
    changed.manufacturers.push(Manufacturer {
        key: "new".into(),
        name: "New".into(),
    });
    let Err(Error::Invalid(msgs)) = import_catalogue(&pool, &changed).await else {
        panic!("expected Invalid");
    };
    assert_eq!(
        msgs,
        [
            "valve type vt-rpe3-06-z11@1 already exists with different content; import it as version 2",
            "part DEMO-06-4-2-S-24DC of vt-demo-4-2@1 already exists with different options",
        ]
    );
    assert_eq!(count(&pool, "manufacturers").await, 2);
    assert_eq!(
        load_catalogue(&pool).await.unwrap().catalogue,
        sorted(fixture())
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn unknown_symbol_ref_is_invalid(pool: PgPool) {
    let mut f = fixture();
    f.valve_types[0].symbol = Some(CatalogueRef::new("sym-missing", 1));
    let Err(Error::Invalid(msgs)) = import_catalogue(&pool, &f).await else {
        panic!("expected Invalid");
    };
    assert_eq!(
        msgs,
        ["valve type vt-rpe3-06-z11@1: unknown symbol sym-missing@1"]
    );
    assert_eq!(count(&pool, "symbol_defs").await, 0);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn bad_key_in_import_is_invalid(pool: PgPool) {
    let mut f = fixture();
    let mut extra = f.symbol_defs[0].clone();
    extra.key = "sym@bad".into();
    f.symbol_defs.push(extra);
    assert_eq!(
        invalid(import_catalogue(&pool, &f).await),
        [r#"symbol key "sym@bad" contains '@'"#]
    );
    assert_eq!(count(&pool, "symbol_defs").await, 0);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn duplicate_manufacturer_name_is_invalid(pool: PgPool) {
    import_catalogue(&pool, &fixture()).await.unwrap();
    let c = Catalogue {
        manufacturers: vec![Manufacturer {
            key: "argo2".into(),
            name: "Argo-Hytos".into(),
        }],
        ..Catalogue::default()
    };
    assert_eq!(
        invalid(import_catalogue(&pool, &c).await),
        [r#"manufacturer name "Argo-Hytos" is already used"#]
    );
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_copies_parts_and_datasheet_forward(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let mut edited = valve(&f, "vt-demo-4-2");
    let v1 = edited.reference();
    let sha = attach_datasheet(&pool, &v1, PDF).await.unwrap();

    edited.code = "  4-2-S-B ".into();
    let v2 = save_valve_type(&pool, &edited).await.unwrap();
    assert_eq!(v2, CatalogueRef::new(&v1.key, 2));

    let loaded = load_catalogue(&pool).await.unwrap();
    let c = &loaded.catalogue;
    assert_eq!(c.valve_type(&v1).unwrap().code, "4-2-S");
    assert_eq!(c.valve_type(&v2).unwrap().code, "4-2-S-B");
    assert_eq!(loaded.datasheets.get(&v2), Some(&sha));
    let parts = |r| {
        c.parts_of(r)
            .map(|p| (p.part_number.clone(), p.options.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(parts(&v1).len(), 2);
    assert_eq!(parts(&v2), parts(&v1));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_from_stale_base_is_refused(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let base = valve(&f, "vt-rpe3-06-z11");
    let mut first = base.clone();
    first.code = "Z11-A".into();
    save_valve_type(&pool, &first).await.unwrap();

    let mut second = base.clone();
    second.code = "Z11-B".into();
    assert_eq!(
        invalid(save_valve_type(&pool, &second).await),
        ["valve type vt-rpe3-06-z11 was changed to version 2 meanwhile; reload and edit again"]
    );
    let c = load_catalogue(&pool).await.unwrap().catalogue;
    let v2 = CatalogueRef::new(&base.key, 2);
    assert_eq!(c.valve_type(&v2).unwrap().code, "Z11-A");
    assert!(c.valve_type(&CatalogueRef::new(&base.key, 3)).is_none());
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn concurrent_saves_from_one_base_let_one_win(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let base = valve(&f, "vt-rpe3-06-z11");
    let saves = (0..4).map(|i| {
        let (pool, mut v) = (pool.clone(), base.clone());
        v.code = format!("Z11-{i}");
        tokio::spawn(async move { save_valve_type(&pool, &v).await })
    });
    let mut ok = 0;
    for save in saves.collect::<Vec<_>>() {
        match save.await.unwrap() {
            Ok(r) => {
                assert_eq!(r, CatalogueRef::new(&base.key, 2));
                ok += 1;
            }
            Err(e) => assert!(matches!(e, Error::Invalid(_)), "{e:?}"),
        }
    }
    assert_eq!(ok, 1);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn save_new_key(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let mut v = valve(&f, "vt-rpe3-06-z11");
    v.version = 0;
    assert_eq!(
        invalid(save_valve_type(&pool, &v).await),
        ["valve type key vt-rpe3-06-z11 already exists"]
    );

    v.key = "vt-new".into();
    assert_eq!(
        save_valve_type(&pool, &v).await.unwrap(),
        CatalogueRef::new("vt-new", 1)
    );

    v.key = "vt-other".into();
    v.manufacturer = "nobody".into();
    assert_eq!(
        invalid(save_valve_type(&pool, &v).await),
        ["unknown manufacturer nobody"]
    );
    v.key = " vt-padded".into();
    assert_eq!(
        invalid(save_valve_type(&pool, &v).await),
        [r#"valve type key " vt-padded" has spaces at the start or end"#]
    );
    v.key = "vt-huge".into();
    v.version = u32::MAX;
    assert!(matches!(
        save_valve_type(&pool, &v).await,
        Err(Error::Invalid(_))
    ));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn datasheet_carries_over_from_lower_versions_only(pool: PgPool) {
    let f = fixture();
    let mut v2 = valve(&f, "vt-rpe3-06-z11");
    v2.version = 2;
    let only_v2 = Catalogue {
        manufacturers: f.manufacturers.clone(),
        symbol_defs: f.symbol_defs.clone(),
        valve_types: vec![v2.clone()],
        parts: vec![],
        blocks: vec![],
    };
    import_catalogue(&pool, &only_v2).await.unwrap();
    let sha = attach_datasheet(&pool, &v2.reference(), PDF).await.unwrap();

    let (mut v1, mut v3) = (v2.clone(), v2.clone());
    v1.version = 1;
    v3.version = 3;
    let more = Catalogue {
        valve_types: vec![v1.clone(), v3.clone()],
        ..only_v2
    };
    import_catalogue(&pool, &more).await.unwrap();
    let loaded = load_catalogue(&pool).await.unwrap();
    assert_eq!(loaded.datasheets.get(&v1.reference()), None);
    assert_eq!(loaded.datasheets.get(&v3.reference()), Some(&sha));
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn attach_dedupes_and_rejects_non_pdf(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let a = f.valve_types[0].reference();
    let b = f.valve_types[1].reference();
    let sha_a = attach_datasheet(&pool, &a, PDF).await.unwrap();
    let sha_b = attach_datasheet(&pool, &b, PDF).await.unwrap();
    assert_eq!(sha_a, sha_b);
    assert_eq!(count(&pool, "assets").await, 1);

    let Err(Error::Invalid(msgs)) = attach_datasheet(&pool, &a, b"hello").await else {
        panic!("expected Invalid");
    };
    assert_eq!(msgs, ["not a PDF file"]);
    let unknown = CatalogueRef::new("vt-missing", 1);
    assert!(matches!(
        attach_datasheet(&pool, &unknown, b"%PDF-other").await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(count(&pool, "assets").await, 1);
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn asset_bytes_round_trip(pool: PgPool) {
    let f = fixture();
    import_catalogue(&pool, &f).await.unwrap();
    let sha = attach_datasheet(&pool, &f.valve_types[0].reference(), PDF)
        .await
        .unwrap();
    assert_eq!(
        asset_bytes(&pool, &sha).await.unwrap().as_deref(),
        Some(PDF)
    );
    assert_eq!(asset_bytes(&pool, "0000").await.unwrap(), None);
}

/// A stored row a stricter build would refuse doesn't block importing others.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn old_problems_dont_block_new_imports(pool: PgPool) {
    sqlx::query(
        "INSERT INTO symbol_defs (key, version, kind, definition) VALUES ('sym-old', 1, \
         'directional_valve', '{\"kind\":\"directional_valve\",\"ports\":[{\"name\":\"A\",\
         \"side\":\"top\"}],\"positions\":[{\"flows\":[[\"A\",\"A\"]]}],\"normal_position\":0}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let r = import_catalogue(&pool, &fixture()).await.unwrap();
    assert!(r.symbol_defs_added > 0);
}
