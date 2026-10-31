//! User administration as the Account tab does it: these tests sign in as real roles and check
//! what migration 0006's functions, and Postgres itself, then let them do.

use hydra_db::{
    Account, Error, account, create_user, delete_user, end_sessions, list_users, set_admin,
    set_can_login, set_password,
};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool};

const ROLES: &str = include_str!("../../../ops/roles.sql");
const PASSWORD: &str = "pilot-pilot";
/// Changes under SASLprep: no-break space, soft hyphen, a ligature and a Roman numeral.
const UNICODE_PASSWORD: &str = "Gr\u{fc}\u{df}e\u{a0}\u{fb01}\u{ad}\u{2168}!";
/// What SASLprep makes of it.
const UNICODE_PREPARED: &str = "Gr\u{fc}\u{df}e fiIX!";
/// A well-formed verifier, of "pencil-pencil".
const VERIFIER: &str = "SCRAM-SHA-256$4096:C0exon3Ed0OrLynq3O3FIQ==$OmfwrcdvLcgsTjHxu5vLcw4o3Im3IUkxIMRD1HNShn4=:d+TmcEBMTc2MYfQ+VOrf3c4SQQJ9SBfumbK27qW1sVo=";
/// Its salt, and its StoredKey:ServerKey.
const SALT: &str = "C0exon3Ed0OrLynq3O3FIQ==";
const KEYS: &str =
    "OmfwrcdvLcgsTjHxu5vLcw4o3Im3IUkxIMRD1HNShn4=:d+TmcEBMTc2MYfQ+VOrf3c4SQQJ9SBfumbK27qW1sVo=";
/// 31 bytes in base64.
const BASE64_31: &str = "OmfwrcdvLcgsTjHxu5vLcw4o3Im3IUkxIMRD1HNShg==";

/// Roles are cluster-wide and roles.sql walks every user: one test at a time.
static CLUSTER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn options(admin: &PgPool, name: &str, password: &str) -> PgConnectOptions {
    (*admin.connect_options())
        .clone()
        .username(name)
        .password(password)
        .application_name("Hydra")
}

async fn run(admin: &PgPool, sql: &str) {
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(admin)
        .await
        .unwrap();
}

/// Sign in as `name`.
async fn login(admin: &PgPool, name: &str, password: &str) -> Result<PgPool, Error> {
    hydra_db::connect_with(options(admin, name, password)).await
}

async fn connection(admin: &PgPool, name: &str, password: &str) -> PgConnection {
    PgConnection::connect_with(&options(admin, name, password))
        .await
        .unwrap()
}

/// Drop `names` (a failed run's too).
async fn drop_roles(admin: &PgPool, names: &[&str]) {
    for name in names.iter().rev() {
        run(admin, &format!("DROP ROLE IF EXISTS {name}")).await;
    }
}

async fn setup(admin: &PgPool, names: &[&str]) -> tokio::sync::MutexGuard<'static, ()> {
    let guard = CLUSTER.lock().await;
    drop_roles(admin, names).await;
    run(admin, ROLES).await;
    guard
}

/// An admin made by the superuser, as ops/roles.sql documents.
async fn boss(admin: &PgPool, name: &str) -> PgPool {
    run(
        admin,
        &format!("CREATE ROLE {name} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_admin"),
    )
    .await;
    run(admin, ROLES).await;
    login(admin, name, PASSWORD).await.unwrap()
}

async fn pg16(admin: &PgPool) -> bool {
    sqlx::query_scalar("SELECT current_setting('server_version_num')::int >= 160000")
        .fetch_one(admin)
        .await
        .unwrap()
}

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(Error::Invalid(problems)) => problems.join("; "),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// What Postgres says to `sql`, which it must refuse.
async fn refused(pool: &PgPool, sql: &str) -> String {
    match sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(pool)
        .await
    {
        Err(e) => e.to_string(),
        Ok(_) => panic!("Postgres allowed {sql}"),
    }
}

async fn is_admin(pool: &PgPool) -> bool {
    account(pool).await.unwrap().admin
}

async fn wrong_login(admin: &PgPool, name: &str, password: &str) -> bool {
    login(admin, name, password)
        .await
        .err()
        .is_some_and(|e| e.is_wrong_login())
}

async fn alive(connection: &mut PgConnection) -> bool {
    sqlx::query("SELECT 1").execute(connection).await.is_ok()
}

async fn exists(admin: &PgPool, name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_roles WHERE rolname = $1)")
        .bind(name)
        .fetch_one(admin)
        .await
        .unwrap()
}

/// Whether `member` has `role`'s privileges or may SET ROLE to it.
async fn has_role(admin: &PgPool, member: &str, role: &str) -> bool {
    sqlx::query_scalar("SELECT pg_has_role($1, $2, 'MEMBER')")
        .bind(member)
        .bind(role)
        .fetch_one(admin)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn only_admins_manage_users(admin: PgPool) {
    let names = ["hydra_t_acc_boss", "hydra_t_acc_eng"];
    let [boss_name, eng_name] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    run(
        &admin,
        &format!("CREATE ROLE {eng_name} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_engineer"),
    )
    .await;
    run(&admin, ROLES).await;
    let eng = login(&admin, eng_name, PASSWORD).await.unwrap();

    let me = |user: &str, admin| Account {
        user: user.into(),
        admin,
    };
    assert_eq!(account(&boss).await.unwrap(), me(boss_name, true));
    assert_eq!(account(&eng).await.unwrap(), me(eng_name, false));

    let only = "only an administrator can manage users";
    assert_eq!(refusal(list_users(&eng).await), only);
    assert_eq!(
        refusal(create_user(&eng, "hydra_t_acc_new", PASSWORD, false).await),
        only
    );
    assert_eq!(
        refusal(set_password(&eng, Some(boss_name), PASSWORD).await),
        only
    );
    assert_eq!(refusal(set_admin(&eng, eng_name, true).await), only);
    assert_eq!(refusal(set_can_login(&eng, boss_name, false).await), only);
    assert_eq!(refusal(end_sessions(&eng, boss_name).await), only);
    assert_eq!(refusal(delete_user(&eng, boss_name).await), only);

    // Calling the functions directly: Postgres refuses, and so would the functions themselves
    // if EXECUTE were ever granted wider than to hydra_admin.
    let calls = [
        "SELECT * FROM hydra_auth.hydra_list_users()".to_string(),
        format!("SELECT hydra_auth.hydra_create_user('hydra_t_acc_new', '{VERIFIER}', true)"),
        format!("SELECT hydra_auth.hydra_set_password('{boss_name}', '{VERIFIER}')"),
        format!("SELECT hydra_auth.hydra_set_admin('{eng_name}', true)"),
        format!("SELECT hydra_auth.hydra_set_can_login('{boss_name}', false)"),
        format!("SELECT hydra_auth.hydra_end_sessions('{boss_name}')"),
        format!("SELECT hydra_auth.hydra_delete_user('{boss_name}')"),
    ];
    for sql in &calls {
        let err = refused(&eng, sql).await;
        assert!(
            err.contains("permission denied for function"),
            "{sql}: {err}"
        );
    }
    run(
        &admin,
        "GRANT EXECUTE ON FUNCTION hydra_auth.hydra_list_users(), \
         hydra_auth.hydra_create_user(text, text, boolean), hydra_auth.hydra_set_password(text, text), \
         hydra_auth.hydra_set_admin(text, boolean), hydra_auth.hydra_set_can_login(text, boolean), \
         hydra_auth.hydra_end_sessions(text), hydra_auth.hydra_delete_user(text) TO hydra_engineer",
    )
    .await;
    for sql in &calls {
        let err = refused(&eng, sql).await;
        assert!(err.contains(only), "{sql}: {err}");
    }
    assert!(!is_admin(&eng).await && is_admin(&boss).await);
    assert!(!exists(&admin, "hydra_t_acc_new").await);

    // ...but anyone changes their own password.
    assert_eq!(
        refusal(set_password(&eng, None, "too-short").await),
        "a password needs at least 10 characters"
    );
    set_password(&eng, None, "engineer-two").await.unwrap();
    login(&admin, eng_name, "engineer-two").await.unwrap();
    assert!(wrong_login(&admin, eng_name, PASSWORD).await);

    for pool in [boss, eng] {
        pool.close().await;
    }
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn new_users_sign_in_with_their_password(admin: PgPool) {
    let names = [
        "hydra_t_new_boss",
        "hydra_t_new_anna",
        "hydra_t_new_carl",
        "hydra_t_new_dora",
    ];
    let [boss_name, anna, carl, dora] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;

    create_user(&boss, anna, UNICODE_PASSWORD, false)
        .await
        .unwrap();
    // The verifier is right: sqlx's own SCRAM exchange accepts the password, SASLprep and all.
    let anna_pool = login(&admin, anna, UNICODE_PASSWORD).await.unwrap();
    login(&admin, anna, UNICODE_PREPARED)
        .await
        .unwrap()
        .close()
        .await;
    assert!(wrong_login(&admin, anna, PASSWORD).await);
    assert!(!is_admin(&anna_pool).await);
    let stored: String = sqlx::query_scalar("SELECT rolpassword FROM pg_authid WHERE rolname = $1")
        .bind(anna)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(stored.starts_with("SCRAM-SHA-256$4096:"), "{stored}");

    if pg16(&admin).await {
        // The caller's createrole_self_grant gives hydra_manager none of a new user's rights...
        let mut conn = connection(&admin, boss_name, PASSWORD).await;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "SET createrole_self_grant = 'set, inherit'; \
             SELECT hydra_auth.hydra_create_user('{dora}', '{VERIFIER}', false)"
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();
        let uses = || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_auth_members WHERE roleid = $1::regrole \
                 AND member = 'hydra_manager'::regrole AND (inherit_option OR set_option)",
            )
            .bind(dora)
            .fetch_one(&admin)
            .await
            .unwrap()
        };
        assert_eq!(uses().await, 0);
        // ...and roles.sql takes back such grants made by hand.
        run(
            &admin,
            &format!("GRANT {dora} TO hydra_manager WITH INHERIT TRUE, SET TRUE"),
        )
        .await;
        assert_eq!(uses().await, 1);
        run(&admin, ROLES).await;
        assert_eq!(uses().await, 0);
        delete_user(&boss, dora).await.unwrap();
    }

    for name in [anna, "postgres", "pg_monitor"] {
        let err = create_user(&boss, name, PASSWORD, false).await.unwrap_err();
        assert!(err.to_string().contains("already exists") || name == "pg_monitor");
    }

    // A new admin can manage users at once, including ones another admin added.
    create_user(&boss, carl, PASSWORD, true).await.unwrap();
    let carl_pool = login(&admin, carl, PASSWORD).await.unwrap();
    assert!(is_admin(&carl_pool).await);
    let users = list_users(&carl_pool).await.unwrap();
    let find = |name| users.iter().find(|u| u.name == name).unwrap();
    assert!(!find(anna).admin && find(anna).can_login);
    assert!(find(carl).admin && find(boss_name).admin);
    assert!(
        !users
            .iter()
            .any(|u| u.name.starts_with("hydra_") && !u.name.starts_with("hydra_t_"))
    );
    set_password(&carl_pool, Some(anna), "set-by-carl")
        .await
        .unwrap();
    login(&admin, anna, "set-by-carl")
        .await
        .unwrap()
        .close()
        .await;

    for pool in [boss, anna_pool, carl_pool] {
        pool.close().await;
    }
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn passwords_change(admin: PgPool) {
    let names = ["hydra_t_pw_boss", "hydra_t_pw_anna"];
    let [boss_name, anna] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    let anna_pool = login(&admin, anna, PASSWORD).await.unwrap();

    set_password(&anna_pool, None, "anna-chose-this")
        .await
        .unwrap();
    login(&admin, anna, "anna-chose-this")
        .await
        .unwrap()
        .close()
        .await;
    assert!(wrong_login(&admin, anna, PASSWORD).await);

    set_password(&boss, Some(anna), "boss-chose-this")
        .await
        .unwrap();
    login(&admin, anna, "boss-chose-this")
        .await
        .unwrap()
        .close()
        .await;
    assert!(wrong_login(&admin, anna, "anna-chose-this").await);

    // Characters, not bytes.
    assert_eq!(
        refusal(set_password(&boss, Some(anna), &"é".repeat(9)).await),
        "a password needs at least 10 characters"
    );
    set_password(&boss, None, &"é".repeat(10)).await.unwrap();
    login(&admin, boss_name, &"é".repeat(10))
        .await
        .unwrap()
        .close()
        .await;

    // Only verifiers: plain text sent as one is refused, whoever sends it.
    for (pool, sql) in [
        (
            &boss,
            format!("SELECT hydra_auth.hydra_set_password('{anna}', 'boss-chose-this')"),
        ),
        (
            &boss,
            "SELECT hydra_auth.hydra_create_user('hydra_t_pw_new', 'pilot-pilot', false)"
                .to_string(),
        ),
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'md5{}')",
                "0".repeat(32)
            ),
        ),
        (
            &boss,
            format!("SELECT hydra_auth.hydra_set_password('{anna}', '{VERIFIER}x')"),
        ),
        (
            &anna_pool,
            "SELECT hydra_auth.hydra_set_own_password('anna-chose-this')".to_string(),
        ),
        // Shaped like a verifier, but Postgres couldn't read it and would keep it as plain text.
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'SCRAM-SHA-256$4096:A${KEYS}')"
            ),
        ),
        (
            &anna_pool,
            format!(
                "SELECT hydra_auth.hydra_set_own_password('SCRAM-SHA-256$4096:QUJDRA=${KEYS}')"
            ),
        ),
        // A salt of 4 bytes, too few and too many iterations, keys of 31 bytes.
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'SCRAM-SHA-256$4096:QUJDRA==${KEYS}')"
            ),
        ),
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'SCRAM-SHA-256$4095:{SALT}${KEYS}')"
            ),
        ),
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'SCRAM-SHA-256$100001:{SALT}${KEYS}')"
            ),
        ),
        (
            &boss,
            format!(
                "SELECT hydra_auth.hydra_set_password('{anna}', 'SCRAM-SHA-256$4096:{SALT}${short}:{short}')",
                short = BASE64_31
            ),
        ),
    ] {
        let err = refused(pool, &sql).await;
        assert!(
            err.contains("must arrive as a SCRAM-SHA-256 verifier"),
            "{sql}: {err}"
        );
    }
    login(&admin, anna, "boss-chose-this")
        .await
        .unwrap()
        .close()
        .await;
    assert!(!exists(&admin, "hydra_t_pw_new").await);

    // Postgres keeps a good verifier as it is.
    run(
        &boss,
        &format!("SELECT hydra_auth.hydra_set_password('{anna}', '{VERIFIER}')"),
    )
    .await;
    let stored: String = sqlx::query_scalar("SELECT rolpassword FROM pg_authid WHERE rolname = $1")
        .bind(anna)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(stored, VERIFIER);
    login(&admin, anna, "pencil-pencil")
        .await
        .unwrap()
        .close()
        .await;

    boss.close().await;
    anna_pool.close().await;
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn admin_rights_come_and_go(admin: PgPool) {
    let names = ["hydra_t_adm_boss", "hydra_t_adm_anna", "hydra_t_adm_dora"];
    let [boss_name, anna, dora] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    let anna_pool = login(&admin, anna, PASSWORD).await.unwrap();

    set_admin(&boss, anna, true).await.unwrap();
    assert!(is_admin(&anna_pool).await);
    // The promoted user manages users, and can promote too.
    create_user(&anna_pool, dora, PASSWORD, false)
        .await
        .unwrap();
    set_password(&anna_pool, Some(dora), "set-by-anna")
        .await
        .unwrap();
    set_admin(&anna_pool, dora, true).await.unwrap();
    set_admin(&anna_pool, dora, false).await.unwrap();
    let users = list_users(&boss).await.unwrap();
    assert!(users.iter().any(|u| u.name == anna && u.admin));
    assert!(users.iter().any(|u| u.name == dora && !u.admin));

    set_admin(&boss, anna, false).await.unwrap();
    assert!(!is_admin(&anna_pool).await);
    assert_eq!(
        refusal(list_users(&anna_pool).await),
        "only an administrator can manage users"
    );
    set_password(&boss, Some(dora), "set-by-boss")
        .await
        .unwrap();
    login(&admin, dora, "set-by-boss")
        .await
        .unwrap()
        .close()
        .await;
    // Asking for what is already so changes nothing.
    set_admin(&boss, anna, false).await.unwrap();
    set_admin(&boss, boss_name, true).await.unwrap();

    // Admin rights granted by hand can only be taken back by hand on PostgreSQL 16+.
    run(&admin, &format!("GRANT hydra_admin TO {anna}")).await;
    let demoted = set_admin(&boss, anna, false).await;
    if pg16(&admin).await {
        assert!(refusal(demoted).contains("ops/roles.sql must run again"));
        assert!(is_admin(&anna_pool).await);
        run(&admin, ROLES).await;
        set_admin(&boss, anna, false).await.unwrap();
    } else {
        demoted.unwrap();
    }
    assert!(!is_admin(&anna_pool).await);

    boss.close().await;
    anna_pool.close().await;
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn sessions_are_listed_and_ended(admin: PgPool) {
    let names = ["hydra_t_ses_boss", "hydra_t_ses_anna"];
    let [boss_name, anna] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();

    let mut first = connection(&admin, anna, PASSWORD).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut first)
        .await
        .unwrap();
    let users = list_users(&boss).await.unwrap();
    let sessions = &users.iter().find(|u| u.name == anna).unwrap().sessions;
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let session = &sessions[0];
    assert_eq!((session.pid, session.application.as_str()), (pid, "Hydra"));
    assert!(session.client.is_some(), "{session:?}");
    let since = session.since.as_bytes();
    assert!(
        since.len() == 16 && since[4] == b'-' && since[10] == b' ' && since[13] == b':',
        "{session:?}"
    );

    let mut second = connection(&admin, anna, PASSWORD).await;
    let users = list_users(&boss).await.unwrap();
    let sessions = &users.iter().find(|u| u.name == anna).unwrap().sessions;
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[0].pid, pid, "oldest first");
    assert_eq!(end_sessions(&boss, anna).await.unwrap(), 2);
    assert!(!alive(&mut first).await && !alive(&mut second).await);
    assert_eq!(end_sessions(&boss, anna).await.unwrap(), 0);

    // Disabling ends sessions and keeps them out; enabling lets them back in.
    let mut third = connection(&admin, anna, PASSWORD).await;
    set_can_login(&boss, anna, false).await.unwrap();
    assert!(!alive(&mut third).await);
    let refused = login(&admin, anna, PASSWORD).await.unwrap_err();
    assert!(!refused.is_wrong_login(), "{refused}");
    let users = list_users(&boss).await.unwrap();
    let user = users.iter().find(|u| u.name == anna).unwrap();
    assert!(!user.can_login && user.sessions.is_empty(), "{user:?}");
    set_can_login(&boss, anna, true).await.unwrap();
    login(&admin, anna, PASSWORD).await.unwrap().close().await;

    boss.close().await;
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn deleting_drops_the_role_and_its_sessions(admin: PgPool) {
    let names = ["hydra_t_del_boss", "hydra_t_del_anna"];
    let [boss_name, anna] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    let mut session = connection(&admin, anna, PASSWORD).await;

    // Ending a deleted user's sessions ends no one else's.
    let oid: i64 = sqlx::query_scalar("SELECT oid::bigint FROM pg_roles WHERE rolname = $1")
        .bind(anna)
        .fetch_one(&admin)
        .await
        .unwrap();
    let err = refused(
        &boss,
        &format!("SELECT hydra_auth.hydra_end_deleted_sessions({oid})"),
    )
    .await;
    assert!(err.contains("that user still exists"), "{err}");
    assert!(alive(&mut session).await);

    delete_user(&boss, anna).await.unwrap();
    assert!(!alive(&mut session).await);
    assert!(!exists(&admin, anna).await);
    assert_eq!(
        refusal(delete_user(&boss, anna).await),
        format!("unknown user {anna}")
    );

    boss.close().await;
    drop_roles(&admin, &names).await;
}

#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn rules_are_checked_first(admin: PgPool) {
    let names = [
        "hydra_t_rul_boss",
        "hydra_t_rul_outsider",
        "hydra_t_rul_anna",
        "hydra_t_rul_owner",
        "hydra_t_rul_power",
    ];
    let [boss_name, outsider, anna, owner, power] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    // Roles in hydra_engineer that are more than users: one owns a table, one may create
    // databases. A superuser put them there; administrators must not take them over.
    run(
        &admin,
        &format!(
            "CREATE ROLE {outsider} LOGIN PASSWORD '{PASSWORD}'; \
             CREATE ROLE {owner} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_engineer; \
             CREATE TABLE hydra_t_rul_owned (id int); \
             ALTER TABLE hydra_t_rul_owned OWNER TO {owner}; \
             CREATE ROLE {power} LOGIN CREATEDB PASSWORD '{PASSWORD}' IN ROLE hydra_engineer"
        ),
    )
    .await;
    run(&admin, ROLES).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();

    let longest = format!("hydra_t_{}", "a".repeat(55));
    let long = format!("{longest}a");
    for bad in [
        "Anna",
        "1anna",
        "an-na",
        "anna smith",
        "änna",
        "",
        "anna\n",
        "x\" SUPERUSER --",
        "anna'; DROP ROLE postgres; --",
        &long,
    ] {
        let message = refusal(create_user(&boss, bad, PASSWORD, false).await);
        assert!(message.contains("is not a user name"), "{bad}: {message}");
        let message = refusal(delete_user(&boss, bad).await);
        assert!(message.contains("is not a user name"), "{bad}: {message}");
    }
    create_user(&boss, &longest, PASSWORD, false).await.unwrap();
    delete_user(&boss, &longest).await.unwrap();
    let pg = "user names can't start with pg_";
    assert_eq!(
        refusal(create_user(&boss, "pg_anna", PASSWORD, false).await),
        pg
    );
    assert_eq!(
        refusal(set_password(&boss, Some("pg_monitor"), PASSWORD).await),
        pg
    );
    // Postgres refuses the first two as role names; the others mean someone else in SQL.
    for name in [
        "public",
        "none",
        "current_user",
        "current_role",
        "session_user",
        "user",
    ] {
        let message = refusal(create_user(&boss, name, PASSWORD, false).await);
        assert!(
            message.contains("means something else"),
            "{name}: {message}"
        );
    }
    for group in ["hydra_engineer", "hydra_admin", "hydra_manager"] {
        let message = format!("{group} is one of Hydra's own roles, not a user");
        assert_eq!(
            refusal(create_user(&boss, group, PASSWORD, false).await),
            message
        );
        assert_eq!(refusal(set_admin(&boss, group, false).await), message);
        assert_eq!(
            refusal(set_password(&boss, Some(group), PASSWORD).await),
            message
        );
        assert_eq!(refusal(delete_user(&boss, group).await), message);
    }

    // Only members of hydra_engineer, and only plain users: never the superuser, any other
    // role, or one with rights of its own.
    for (name, message) in [
        (
            "hydra_t_rul_ghost",
            "unknown user hydra_t_rul_ghost".to_string(),
        ),
        ("postgres", "unknown user postgres".to_string()),
        (outsider, format!("unknown user {outsider}")),
        (
            power,
            format!("{power} has database rights beyond a Hydra user's"),
        ),
    ] {
        let starts = |r: String| assert!(r.starts_with(&message), "{name}: {r}");
        starts(refusal(set_password(&boss, Some(name), PASSWORD).await));
        starts(refusal(set_admin(&boss, name, true).await));
        starts(refusal(set_admin(&boss, name, false).await));
        starts(refusal(set_can_login(&boss, name, false).await));
        starts(refusal(set_can_login(&boss, name, true).await));
        starts(refusal(end_sessions(&boss, name).await));
        starts(refusal(delete_user(&boss, name).await));
    }
    for name in [outsider, owner, power] {
        login(&admin, name, PASSWORD).await.unwrap().close().await;
    }
    // Owning objects stops giving rights and deleting, never taking rights away.
    let beyond = format!("{owner} has database rights beyond a Hydra user's");
    let starts = |r: String| assert!(r.starts_with(&beyond), "{r}");
    starts(refusal(set_password(&boss, Some(owner), PASSWORD).await));
    starts(refusal(set_admin(&boss, owner, true).await));
    starts(refusal(set_can_login(&boss, owner, true).await));
    let message = refusal(delete_user(&boss, owner).await);
    assert!(
        message.starts_with(&format!("{owner} owns database objects")),
        "{message}"
    );
    set_admin(&boss, owner, false).await.unwrap();
    end_sessions(&boss, owner).await.unwrap();
    set_can_login(&boss, owner, false).await.unwrap();
    assert!(login(&admin, owner, PASSWORD).await.is_err());

    assert_eq!(
        refusal(delete_user(&boss, boss_name).await),
        "you can't delete yourself"
    );
    assert_eq!(
        refusal(set_can_login(&boss, boss_name, false).await),
        "you can't disable yourself"
    );
    assert_eq!(
        refusal(set_admin(&boss, boss_name, false).await),
        "you can't remove your own administrator rights"
    );
    assert_eq!(
        refusal(end_sessions(&boss, boss_name).await),
        "you can't end your own sessions"
    );
    assert!(is_admin(&boss).await);
    login(&admin, boss_name, PASSWORD)
        .await
        .unwrap()
        .close()
        .await;

    let short = "a password needs at least 10 characters";
    assert_eq!(
        refusal(create_user(&boss, "hydra_t_rul_new", "123456789", false).await),
        short
    );
    assert_eq!(
        refusal(set_password(&boss, Some(anna), "123456789").await),
        short
    );
    assert_eq!(refusal(set_password(&boss, None, "").await), short);
    // A control character SASLprep refuses: sqlx couldn't sign in with it.
    let message = refusal(set_password(&boss, Some(anna), "pilot\u{7}pilot").await);
    assert!(message.contains("can't be used"), "{message}");
    login(&admin, anna, PASSWORD).await.unwrap().close().await;

    boss.close().await;
    run(&admin, "DROP TABLE hydra_t_rul_owned").await;
    drop_roles(&admin, &names).await;
}

/// Every admin manages every other admin and user, on PostgreSQL 15 and 16+ alike.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn admins_manage_each_other(admin: PgPool) {
    let names = [
        "hydra_t_all_boss1",
        "hydra_t_all_boss2",
        "hydra_t_all_old",
        "hydra_t_all_anna",
        "hydra_t_all_sue",
    ];
    let [boss1_name, boss2_name, old, anna, sue] = names;
    let _cluster = setup(&admin, &names).await;
    let boss1 = boss(&admin, boss1_name).await;
    create_user(&boss1, boss2_name, PASSWORD, true)
        .await
        .unwrap();
    let boss2 = login(&admin, boss2_name, PASSWORD).await.unwrap();
    // Added by hand: on 16+ administrators can change it only once roles.sql has run again.
    run(
        &admin,
        &format!("CREATE ROLE {old} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_engineer"),
    )
    .await;
    let early = set_password(&boss2, Some(old), "set-by-boss2").await;
    if pg16(&admin).await {
        assert!(refusal(early).contains("ops/roles.sql must run again"));
    } else {
        early.unwrap();
    }
    run(&admin, ROLES).await;
    create_user(&boss1, anna, PASSWORD, false).await.unwrap();
    assert!(is_admin(&admin).await, "the superuser counts as an admin");
    create_user(&admin, sue, PASSWORD, false).await.unwrap();

    for (by, user, was_admin) in [
        (&boss2, old, false),
        (&boss2, anna, false),
        (&boss2, sue, false),
        (&boss2, boss1_name, true),
        (&boss1, boss2_name, true),
    ] {
        set_password(by, Some(user), "set-by-other").await.unwrap();
        let mut session = connection(&admin, user, "set-by-other").await;
        // Admins also have their own pool's sessions open.
        assert!(end_sessions(by, user).await.unwrap() >= 1, "{user}");
        assert!(!alive(&mut session).await);
        set_can_login(by, user, false).await.unwrap();
        assert!(login(&admin, user, "set-by-other").await.is_err());
        set_can_login(by, user, true).await.unwrap();
        set_admin(by, user, !was_admin).await.unwrap();
        let pool = login(&admin, user, "set-by-other").await.unwrap();
        assert_eq!(is_admin(&pool).await, !was_admin, "{user}");
        set_admin(by, user, was_admin).await.unwrap();
        assert_eq!(is_admin(&pool).await, was_admin, "{user}");
        pool.close().await;
        // Their pool reconnects with this one.
        set_password(by, Some(user), PASSWORD).await.unwrap();
    }
    for user in [old, anna, sue] {
        delete_user(&boss1, user).await.unwrap();
        assert!(!exists(&admin, user).await, "{user}");
    }
    delete_user(&boss2, boss1_name).await.unwrap();
    assert!(!exists(&admin, boss1_name).await);

    boss1.close().await;
    boss2.close().await;
    drop_roles(&admin, &names).await;
}

/// What administrators can do outside the functions: nothing an engineer can't.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn postgres_refuses_admins_directly(admin: PgPool) {
    let names = [
        "hydra_t_raw_boss",
        "hydra_t_raw_anna",
        "hydra_t_raw_outsider",
    ];
    let [boss_name, anna, outsider] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    run(
        &admin,
        &format!("CREATE ROLE {outsider} LOGIN PASSWORD '{PASSWORD}'"),
    )
    .await;

    for sql in [
        format!("ALTER ROLE {outsider} PASSWORD 'taken-over'"),
        format!("ALTER ROLE {anna} PASSWORD 'taken-over'"),
        format!("ALTER ROLE {anna} NOLOGIN"),
        format!("DROP ROLE {anna}"),
        "CREATE ROLE hydra_t_raw_new LOGIN".to_string(),
        format!("GRANT pg_execute_server_program TO {boss_name}"),
        format!("GRANT pg_read_server_files TO {boss_name}"),
        format!("GRANT hydra_engineer TO {outsider}"),
        format!("GRANT hydra_admin TO {anna}"),
        format!("GRANT {anna} TO {boss_name}"),
        format!("SET ROLE {anna}"),
        "SET ROLE hydra_manager".to_string(),
        "SET ROLE postgres".to_string(),
        format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '{anna}'"),
        // The helper the functions use is theirs alone.
        format!("SELECT hydra_auth.hydra_user_check('{anna}', 'give', NULL)"),
    ] {
        let mut anna_session = connection(&admin, anna, PASSWORD).await;
        let err = refused(&boss, &sql).await;
        assert!(
            err.contains("permission denied")
                || err.contains("must have admin option")
                || err.contains("must be able to SET ROLE")
                || err.contains("must be a member of the role"),
            "{sql}: {err}"
        );
        assert!(alive(&mut anna_session).await, "{sql}");
    }
    login(&admin, outsider, PASSWORD)
        .await
        .unwrap()
        .close()
        .await;
    login(&admin, anna, PASSWORD).await.unwrap().close().await;

    // Crafted arguments are only ever names.
    for sql in [
        format!(
            "SELECT hydra_auth.hydra_create_user('a\"; ALTER ROLE {boss_name} SUPERUSER; --', '{VERIFIER}', true)"
        ),
        format!("SELECT hydra_auth.hydra_set_password('{anna} SUPERUSER', '{VERIFIER}')"),
        format!("SELECT hydra_auth.hydra_set_password('{anna}', '{VERIFIER}'' SUPERUSER --')"),
        "SELECT hydra_auth.hydra_set_admin('pg_execute_server_program', true)".to_string(),
        format!("SELECT hydra_auth.hydra_delete_user('{outsider}')"),
    ] {
        let err = refused(&boss, &sql).await;
        assert!(
            err.contains("is not a user name")
                || err.contains("SCRAM-SHA-256 verifier")
                || err.contains("start with pg_")
                || err.contains("unknown user"),
            "{sql}: {err}"
        );
    }

    let superuser: bool = sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = $1")
        .bind(boss_name)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(!superuser);
    // The manager can't sign in, and administrators hold no role but hydra_admin's.
    let (manager_login, manager_password): (bool, bool) = sqlx::query_as(
        "SELECT rolcanlogin, rolpassword IS NOT NULL FROM pg_authid WHERE rolname = 'hydra_manager'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!manager_login && !manager_password);
    for role in [
        anna,
        "hydra_manager",
        "pg_signal_backend",
        "pg_read_all_stats",
    ] {
        assert!(!has_role(&admin, boss_name, role).await, "{role}");
        assert!(!has_role(&admin, "hydra_admin", role).await, "{role}");
    }
    let admin_options: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_auth_members a JOIN pg_roles m ON m.oid = a.member \
         WHERE a.admin_option AND m.rolname <> 'hydra_manager' AND NOT m.rolsuper \
           AND a.roleid IN ('hydra_engineer'::regrole, 'hydra_admin'::regrole)",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(admin_options, 0);

    boss.close().await;
    drop_roles(&admin, &names).await;
}

/// Revoke `role` from `member` whoever granted it (PostgreSQL 16+ keeps one grant each).
async fn revoke_all(admin: &PgPool, role: &str, member: &str) {
    run(
        admin,
        &format!(
            "DO $$ DECLARE g regrole; BEGIN \
               FOR g IN SELECT grantor::regrole FROM pg_auth_members \
                        WHERE roleid = '{role}'::regrole AND member = '{member}'::regrole LOOP \
                 IF current_setting('server_version_num')::int >= 160000 THEN \
                   EXECUTE format('REVOKE {role} FROM {member} GRANTED BY %s CASCADE', g); \
                 ELSE EXECUTE 'REVOKE {role} FROM {member}'; END IF; \
               END LOOP; END $$"
        ),
    )
    .await;
}

/// ops/roles.sql takes back what its earlier version granted (CREATEROLE admins managing users
/// through hydra_admin), and adopts no role that is more than a user.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn roles_sql_undoes_the_old_setup(admin: PgPool) {
    let names = ["hydra_t_old_boss", "hydra_t_old_anna", "hydra_t_old_carl"];
    let [boss_name, anna, carl] = names;
    let _cluster = setup(&admin, &names).await;
    // The earlier script's grants, an admin it documented, a user that admin added, and a
    // system role someone slipped into hydra_engineer.
    run(
        &admin,
        &format!(
            "GRANT hydra_engineer TO hydra_admin WITH ADMIN OPTION; \
             GRANT pg_signal_backend, pg_read_all_stats TO hydra_admin; \
             CREATE ROLE {boss_name} LOGIN CREATEROLE PASSWORD '{PASSWORD}' IN ROLE hydra_engineer; \
             GRANT hydra_admin TO {boss_name} WITH ADMIN OPTION; \
             SET ROLE {boss_name}; \
             CREATE ROLE {anna} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_engineer; \
             GRANT {anna} TO hydra_admin WITH ADMIN OPTION; \
             RESET ROLE; \
             GRANT hydra_engineer TO pg_monitor"
        ),
    )
    .await;
    assert!(has_role(&admin, "hydra_admin", anna).await);
    run(&admin, ROLES).await;

    for role in [anna, "pg_signal_backend", "pg_read_all_stats", "pg_monitor"] {
        assert!(!has_role(&admin, "hydra_admin", role).await, "{role}");
        assert!(!has_role(&admin, boss_name, role).await, "{role}");
    }
    assert!(!has_role(&admin, "hydra_manager", "pg_monitor").await);
    let admin_options: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_auth_members a JOIN pg_roles m ON m.oid = a.member \
         WHERE a.admin_option AND m.rolname <> 'hydra_manager' AND NOT m.rolsuper \
           AND a.roleid IN ('hydra_engineer'::regrole, 'hydra_admin'::regrole, $1::regrole)",
    )
    .bind(anna)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(admin_options, 0);

    // boss is still an admin, without the CREATEROLE that on PostgreSQL 15 is as good as
    // superuser.
    let createrole: bool =
        sqlx::query_scalar("SELECT rolcreaterole FROM pg_roles WHERE rolname = $1")
            .bind(boss_name)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(!createrole);
    let boss = login(&admin, boss_name, PASSWORD).await.unwrap();
    assert!(is_admin(&boss).await);
    for sql in [
        format!("SET ROLE {anna}"),
        format!("GRANT pg_execute_server_program TO {boss_name}"),
        format!("ALTER ROLE {anna} PASSWORD 'taken-over'"),
    ] {
        let err = refused(&boss, &sql).await;
        assert!(
            err.contains("permission denied") || err.contains("must have admin option"),
            "{sql}: {err}"
        );
    }
    create_user(&boss, carl, PASSWORD, true).await.unwrap();
    let carl_pool = login(&admin, carl, PASSWORD).await.unwrap();
    set_password(&carl_pool, Some(anna), "set-by-carl")
        .await
        .unwrap();
    login(&admin, anna, "set-by-carl")
        .await
        .unwrap()
        .close()
        .await;
    assert_eq!(
        refusal(set_admin(&carl_pool, "pg_monitor", true).await),
        "user names can't start with pg_"
    );
    set_admin(&carl_pool, boss_name, false).await.unwrap();
    assert!(!is_admin(&boss).await);
    boss.close().await;
    delete_user(&carl_pool, boss_name).await.unwrap();
    delete_user(&carl_pool, anna).await.unwrap();

    carl_pool.close().await;
    revoke_all(&admin, "hydra_engineer", "pg_monitor").await;
    drop_roles(&admin, &names).await;
}

/// Authors are the signed-in role, even after SET ROLE to someone else.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn set_role_does_not_change_authors(admin: PgPool) {
    let names = ["hydra_t_who_anna", "hydra_t_who_bob"];
    let [anna, bob] = names;
    let _cluster = setup(&admin, &names).await;
    create_user(&admin, anna, PASSWORD, false).await.unwrap();
    create_user(&admin, bob, PASSWORD, false).await.unwrap();
    run(&admin, &format!("GRANT {anna} TO {bob}")).await;
    let mut conn = connection(&admin, bob, PASSWORD).await;
    let authors: Vec<String> = sqlx::raw_sql(AssertSqlSafe(format!(
        "SET ROLE {anna}; \
         WITH c AS (INSERT INTO circuits (name, owner) VALUES ('who', '{anna}') RETURNING id, owner), \
              r AS (INSERT INTO circuit_revisions (circuit_id, number, document, created_by) \
                    SELECT id, 1, '{{}}', '{anna}' FROM c RETURNING created_by), \
              p AS (INSERT INTO catalogue_proposals (proposal, created_by) \
                    VALUES ('{{}}', '{anna}') RETURNING id, created_by) \
         SELECT owner FROM c UNION ALL SELECT created_by FROM r UNION ALL SELECT created_by FROM p"
    )))
    .fetch_all(&mut conn)
    .await
    .unwrap()
    .iter()
    .map(|row| sqlx::Row::get(row, 0))
    .collect();
    assert_eq!(authors, [bob, bob, bob]);
    let reviewer: String = sqlx::query_scalar(
        "UPDATE catalogue_proposals SET status = 'rejected', reviewed_by = current_user \
         RETURNING reviewed_by",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(reviewer, bob);
    conn.close().await.unwrap();
    drop_roles(&admin, &names).await;
}

/// A database owner who isn't a superuser owns schema public on PostgreSQL 15+, but nothing in
/// hydra_auth: what administrators run stays migration 0006's.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn database_owner_cannot_change_the_functions(admin: PgPool) {
    let names = ["hydra_t_dbo_boss", "hydra_t_dbo_anna"];
    let [boss_name, anna] = names;
    let dbo = "hydra_t_dbo";
    let _cluster = setup(&admin, &names).await;
    let disown = format!(
        "DO $$ BEGIN IF to_regrole('{dbo}') IS NOT NULL THEN \
           REASSIGN OWNED BY {dbo} TO CURRENT_USER; DROP OWNED BY {dbo}; DROP ROLE {dbo}; \
         END IF; END $$"
    );
    run(&admin, &disown).await;
    boss(&admin, boss_name).await.close().await;
    let db: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&admin)
        .await
        .unwrap();
    run(
        &admin,
        &format!(
            "CREATE ROLE {dbo} LOGIN PASSWORD '{PASSWORD}'; ALTER DATABASE \"{db}\" OWNER TO {dbo}"
        ),
    )
    .await;

    // ops/roles.sql changes nothing unless a superuser runs it.
    let mut conn = connection(&admin, dbo, PASSWORD).await;
    let err = sqlx::raw_sql(ROLES).execute(&mut conn).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("ops/roles.sql must run as a database superuser"),
        "{err}"
    );
    conn.close().await.unwrap();

    let owner = login(&admin, dbo, PASSWORD).await.unwrap();
    for sql in [
        "DROP FUNCTION hydra_auth.hydra_user_check(text, text, text)",
        "DROP SCHEMA hydra_auth CASCADE",
        "ALTER SCHEMA hydra_auth RENAME TO hydra_gone",
        "ALTER SCHEMA hydra_auth OWNER TO hydra_t_dbo",
        "ALTER FUNCTION hydra_auth.hydra_verifier(text) OWNER TO hydra_t_dbo",
        "CREATE OR REPLACE FUNCTION hydra_auth.hydra_user_check(username text, action text, \
         refusal text) RETURNS void LANGUAGE sql AS ''",
        "CREATE FUNCTION hydra_auth.hydra_verifier(verifier varchar) RETURNS text \
         LANGUAGE sql AS 'SELECT $1'",
    ] {
        let err = refused(&owner, sql).await;
        assert!(
            err.contains("must be owner") || err.contains("permission denied"),
            "{sql}: {err}"
        );
    }
    // Same names in public, found first by everyone's search_path, are never called (the
    // owner of public could drop and replace anything there).
    let planted = "LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'planted'; END $$";
    run(
        &owner,
        &format!(
            "DROP FUNCTION IF EXISTS public.hydra_user_check(text, text, text); \
             CREATE FUNCTION public.hydra_user_check(text, text, text) RETURNS void {planted}; \
             CREATE FUNCTION public.hydra_keep_an_admin(text) RETURNS void {planted}; \
             CREATE FUNCTION public.hydra_verifier(text) RETURNS text {planted}; \
             CREATE FUNCTION public.hydra_create_user(text, text, boolean) RETURNS void {planted}; \
             CREATE FUNCTION public.hydra_set_password(text, text) RETURNS void {planted}; \
             CREATE FUNCTION public.hydra_delete_user(text) RETURNS oid {planted}; \
             ALTER DATABASE \"{db}\" SET search_path = public, pg_catalog"
        ),
    )
    .await;
    owner.close().await;

    let boss = login(&admin, boss_name, PASSWORD).await.unwrap();
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    set_password(&boss, Some(anna), "set-by-boss")
        .await
        .unwrap();
    set_admin(&boss, anna, true).await.unwrap();
    set_admin(&boss, anna, false).await.unwrap();
    set_can_login(&boss, anna, false).await.unwrap();
    set_can_login(&boss, anna, true).await.unwrap();
    login(&admin, anna, "set-by-boss")
        .await
        .unwrap()
        .close()
        .await;
    assert!(
        list_users(&boss)
            .await
            .unwrap()
            .iter()
            .any(|u| u.name == anna)
    );
    delete_user(&boss, anna).await.unwrap();
    assert!(!exists(&admin, anna).await);

    let owners: Vec<String> = sqlx::query_scalar(
        "SELECT nspowner::regrole::text FROM pg_namespace WHERE nspname = 'hydra_auth' \
         UNION ALL SELECT proowner::regrole::text FROM pg_proc \
         WHERE pronamespace = 'hydra_auth'::regnamespace",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert!(
        owners.len() > 10 && owners.iter().all(|o| o == "hydra_manager"),
        "{owners:?}"
    );
    for role in [dbo, boss_name, "hydra_engineer", "public"] {
        let create: bool =
            sqlx::query_scalar("SELECT has_schema_privilege($1, 'hydra_auth', 'CREATE')")
                .bind(role)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(!create, "{role}");
    }

    boss.close().await;
    run(&admin, &disown).await;
    drop_roles(&admin, &names).await;
}

/// Anyone can own a temporary table, and could create large objects before roles.sql took that
/// away: neither keeps administrators from disabling a user or ending their sessions.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn owning_objects_does_not_protect_a_user(admin: PgPool) {
    let names = ["hydra_t_obj_boss", "hydra_t_obj_anna", "hydra_t_obj_bert"];
    let [boss_name, anna, bert] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    create_user(&boss, anna, PASSWORD, false).await.unwrap();
    let mut session = connection(&admin, anna, PASSWORD).await;
    let err = sqlx::raw_sql("SELECT lo_create(0)")
        .execute(&mut session)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("permission denied"), "{err}");
    // Large objects made earlier, one by bert, added by hand and adopted all the same.
    run(
        &admin,
        &format!(
            "CREATE ROLE {bert} LOGIN PASSWORD '{PASSWORD}' IN ROLE hydra_engineer; \
             DO $$ BEGIN \
               EXECUTE format('ALTER LARGE OBJECT %s OWNER TO {anna}', lo_create(0)); \
               EXECUTE format('ALTER LARGE OBJECT %s OWNER TO {bert}', lo_create(0)); \
             END $$"
        ),
    )
    .await;
    run(&admin, ROLES).await;
    sqlx::raw_sql("CREATE TEMP TABLE kept (id int)")
        .execute(&mut session)
        .await
        .unwrap();

    let beyond = |name: &str| format!("{name} has database rights beyond a Hydra user's");
    for name in [anna, bert] {
        let starts = |r: String| assert!(r.starts_with(&beyond(name)), "{r}");
        starts(refusal(set_password(&boss, Some(name), PASSWORD).await));
        starts(refusal(set_admin(&boss, name, true).await));
        let message = refusal(delete_user(&boss, name).await);
        assert!(
            message.starts_with(&format!("{name} owns database objects")),
            "{message}"
        );
    }

    assert_eq!(end_sessions(&boss, anna).await.unwrap(), 1);
    assert!(!alive(&mut session).await);
    let mut session = connection(&admin, anna, PASSWORD).await;
    sqlx::raw_sql("CREATE TEMP TABLE kept (id int)")
        .execute(&mut session)
        .await
        .unwrap();
    set_can_login(&boss, anna, false).await.unwrap();
    assert!(!alive(&mut session).await);
    let refused_login = login(&admin, anna, PASSWORD).await.unwrap_err();
    assert!(refused_login.is_login_refused(), "{refused_login}");
    set_admin(&boss, bert, false).await.unwrap();
    set_can_login(&boss, bert, false).await.unwrap();

    // Once they own nothing, privileges of their own still count.
    run(
        &admin,
        &format!(
            "SELECT lo_unlink(oid) FROM pg_largeobject_metadata \
             WHERE lomowner IN ('{anna}'::regrole, '{bert}'::regrole); \
             GRANT SELECT ON parts TO {anna}"
        ),
    )
    .await;
    assert!(refusal(set_can_login(&boss, anna, true).await).starts_with(&beyond(anna)));
    run(&admin, &format!("REVOKE SELECT ON parts FROM {anna}")).await;
    set_can_login(&boss, anna, true).await.unwrap();
    login(&admin, anna, PASSWORD).await.unwrap().close().await;
    delete_user(&boss, anna).await.unwrap();
    delete_user(&boss, bert).await.unwrap();

    boss.close().await;
    drop_roles(&admin, &names).await;
}

/// When a statement that carries a verifier fails, the error doesn't carry the statement.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn errors_do_not_carry_the_verifier(admin: PgPool) {
    let names = ["hydra_t_leak_boss", "hydra_t_leak_anna"];
    let [boss_name, anna] = names;
    let _cluster = setup(&admin, &names).await;
    let boss = boss(&admin, boss_name).await;
    // Another session creates anna between the function's check and its CREATE ROLE.
    let mut other = PgConnection::connect_with(&admin.connect_options())
        .await
        .unwrap();
    sqlx::raw_sql(AssertSqlSafe(format!("BEGIN; CREATE ROLE {anna}")))
        .execute(&mut other)
        .await
        .unwrap();
    let creating = {
        let boss = boss.clone();
        tokio::spawn(async move { create_user(&boss, anna, PASSWORD, false).await })
    };
    let mut waits = 0;
    while !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT FROM pg_stat_activity WHERE usename = $1 AND wait_event_type = 'Lock')",
    )
    .bind(boss_name)
    .fetch_one(&admin)
    .await
    .unwrap()
    {
        waits += 1;
        assert!(waits < 100, "create_user never waited for the other session");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    sqlx::raw_sql("COMMIT").execute(&mut other).await.unwrap();
    other.close().await.unwrap();
    let err = creating.await.unwrap().unwrap_err();
    let shown = format!("{err} {err:?}");
    assert!(
        shown.contains("already exists") || shown.contains("duplicate key"),
        "{shown}"
    );
    assert!(!shown.contains("SCRAM-SHA-256$"), "{shown}");

    boss.close().await;
    drop_roles(&admin, &names).await;
}

/// Hydra keeps an administrator who can sign in, even when two remove each other at once.
#[sqlx::test(migrator = "hydra_db::MIGRATOR")]
#[ignore = "needs DATABASE_URL (Postgres)"]
async fn the_last_admin_stays(admin: PgPool) {
    let names = ["hydra_t_last_boss1", "hydra_t_last_boss2"];
    let [boss1_name, boss2_name] = names;
    let _cluster = setup(&admin, &names).await;
    let admins = "SELECT count(*) FROM pg_roles r \
                  WHERE r.rolcanlogin AND NOT r.rolsuper AND pg_has_role(r.oid, 'hydra_admin', 'MEMBER')";
    let others: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "{admins} AND r.rolname NOT LIKE 'hydra\\_t\\_last\\_%'"
    )))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        others, 0,
        "this test needs a cluster with no other administrators"
    );
    let boss1 = boss(&admin, boss1_name).await;

    // The superuser counts as an administrator here, but isn't one Hydra keeps.
    let last = "Hydra needs at least one administrator";
    assert_eq!(refusal(set_admin(&admin, boss1_name, false).await), last);
    assert_eq!(
        refusal(set_can_login(&admin, boss1_name, false).await),
        last
    );
    assert_eq!(refusal(delete_user(&admin, boss1_name).await), last);
    create_user(&boss1, boss2_name, PASSWORD, true)
        .await
        .unwrap();
    // One who can't sign in doesn't count.
    set_can_login(&boss1, boss2_name, false).await.unwrap();
    assert_eq!(refusal(set_admin(&admin, boss1_name, false).await), last);
    set_can_login(&boss1, boss2_name, true).await.unwrap();

    // Two administrators remove each other at once: both wait for the lock held here, then
    // one of them stays.
    let boss2 = login(&admin, boss2_name, PASSWORD).await.unwrap();
    let mut lock = PgConnection::connect_with(&admin.connect_options())
        .await
        .unwrap();
    sqlx::raw_sql("SELECT pg_advisory_lock(448682234465)")
        .execute(&mut lock)
        .await
        .unwrap();
    let demote = |by: &PgPool, name: &'static str| {
        let by = by.clone();
        tokio::spawn(async move { set_admin(&by, name, false).await })
    };
    let (one, two) = (demote(&boss1, boss2_name), demote(&boss2, boss1_name));
    let mut waits = 0;
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted",
    )
    .fetch_one(&admin)
    .await
    .unwrap()
        < 2
    {
        waits += 1;
        assert!(waits < 100, "the demotions never waited for each other");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    lock.close().await.unwrap();
    let (one, two) = (one.await.unwrap(), two.await.unwrap());
    assert!(one.is_ok() != two.is_ok(), "{one:?} {two:?}");
    let left: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "{admins} AND r.rolname LIKE 'hydra\\_t\\_last\\_%'"
    )))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(left, 1);

    boss1.close().await;
    boss2.close().await;
    drop_roles(&admin, &names).await;
}
