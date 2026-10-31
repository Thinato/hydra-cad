//! Hydra users: Postgres login roles in the group role `hydra_engineer` (ops/roles.sql).
//! Administrators are members of `hydra_admin`, with no Postgres rights of their own: they
//! manage users through migration 0006's functions, which run as `hydra_manager` and check
//! everything themselves. Passwords travel only as SCRAM verifiers, in bind parameters.

use sqlx::PgPool;
use sqlx::postgres::types::Oid;

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub user: String,
    pub admin: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub pid: i32,
    /// application_name: "Hydra" for the app, "hydra-mcp" for the MCP server, else whatever.
    pub application: String,
    /// Client address, when Postgres reports it.
    pub client: Option<String>,
    /// Connected since, "YYYY-MM-DD HH:MM" in the session time zone (UTC for sqlx clients).
    pub since: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub name: String,
    pub admin: bool,
    pub can_login: bool,
    /// This role's open connections to the server, oldest first.
    pub sessions: Vec<Session>,
}

pub const MIN_PASSWORD_CHARS: usize = 10;

/// SCRAM iterations, as Postgres and psql use by default.
const ITERATIONS: u32 = 4096;

/// SQLSTATE of the functions' refusals.
const REFUSED: &str = "H0001";

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(vec![message.into()])
}

/// The SCRAM-SHA-256 verifier Postgres stores for `password`, so the password itself never
/// reaches the server, its logs or pg_stat_statements.
fn verifier(password: &str) -> Result<String, Error> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(invalid(format!(
            "a password needs at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    // sqlx refuses to sign in with a password SASLprep rejects, so don't set one.
    let prepared = stringprep::saslprep(password)
        .map_err(|_| invalid("the password has characters that can't be used in a password"))?;
    Ok(scram(&prepared, &rand::random::<[u8; 16]>()))
}

/// RFC 5802 with RFC 7677's SHA-256, in Postgres's `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`.
fn scram(prepared: &str, salt: &[u8]) -> String {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    use hmac::{Hmac, KeyInit, Mac};
    use sha2_hmac::{Digest, Sha256};

    let hmac = |key: &[u8], parts: &[&[u8]]| {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes keys of any length");
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes()
    };
    // SaltedPassword = Hi(password, salt, i): PBKDF2-HMAC-SHA-256, one 32-byte block.
    let mut u = hmac(prepared.as_bytes(), &[salt, &1u32.to_be_bytes()]);
    let mut salted = u;
    for _ in 1..ITERATIONS {
        u = hmac(prepared.as_bytes(), &[&u]);
        salted.iter_mut().zip(&u).for_each(|(s, u)| *s ^= u);
    }
    let stored_key = Sha256::digest(hmac(&salted, &[b"Client Key"]));
    let server_key = hmac(&salted, &[b"Server Key"]);
    format!(
        "SCRAM-SHA-256${ITERATIONS}:{}${}:{}",
        BASE64_STANDARD.encode(salt),
        BASE64_STANDARD.encode(stored_key),
        BASE64_STANDARD.encode(server_key)
    )
}

/// Postgres's refusal in plain words, or the error as it is.
fn refused(e: sqlx::Error) -> Error {
    match e.as_database_error() {
        Some(d) if d.code().as_deref() == Some(REFUSED) => invalid(d.message()),
        _ => e.into(),
    }
}

/// Who is signed in, and whether they may manage users.
pub async fn account(pool: &PgPool) -> Result<Account, Error> {
    let (user, admin) = sqlx::query_as(
        "SELECT current_user::text, \
                COALESCE(pg_has_role(to_regrole('hydra_admin'), 'MEMBER'), false)",
    )
    .fetch_one(pool)
    .await?;
    Ok(Account { user, admin })
}

/// Only administrators may call the functions, so others hear why here.
async fn require_admin(pool: &PgPool) -> Result<(), Error> {
    if !account(pool).await?.admin {
        return Err(invalid("only an administrator can manage users"));
    }
    Ok(())
}

/// Members of hydra_engineer, by name. Admins only.
pub async fn list_users(pool: &PgPool) -> Result<Vec<User>, Error> {
    require_admin(pool).await?;
    type Row = (
        String,
        bool,
        bool,
        Option<i32>,
        String,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as("SELECT * FROM hydra_auth.hydra_list_users()")
        .fetch_all(pool)
        .await
        .map_err(refused)?;
    let mut users: Vec<User> = Vec::new();
    for (name, admin, can_login, pid, application, client, since) in rows {
        if users.last().is_none_or(|u| u.name != name) {
            users.push(User {
                name,
                admin,
                can_login,
                sessions: Vec::new(),
            });
        }
        if let (Some(pid), Some(since), Some(user)) = (pid, since, users.last_mut()) {
            user.sessions.push(Session {
                pid,
                application,
                client,
                since,
            });
        }
    }
    Ok(users)
}

/// A new login role in hydra_engineer (and hydra_admin when `admin`). Admins only.
pub async fn create_user(
    pool: &PgPool,
    name: &str,
    password: &str,
    admin: bool,
) -> Result<(), Error> {
    require_admin(pool).await?;
    sqlx::query("SELECT hydra_auth.hydra_create_user($1, $2, $3)")
        .bind(name)
        .bind(verifier(password)?)
        .bind(admin)
        .execute(pool)
        .await
        .map_err(refused)?;
    Ok(())
}

/// `name: None` changes the signed-in user's own password (anyone); `Some` is admins only.
pub async fn set_password(pool: &PgPool, name: Option<&str>, password: &str) -> Result<(), Error> {
    let query = match name {
        None => sqlx::query("SELECT hydra_auth.hydra_set_own_password($1)"),
        Some(name) => {
            require_admin(pool).await?;
            sqlx::query("SELECT hydra_auth.hydra_set_password($1, $2)").bind(name)
        }
    };
    query
        .bind(verifier(password)?)
        .execute(pool)
        .await
        .map_err(refused)?;
    Ok(())
}

pub async fn set_admin(pool: &PgPool, name: &str, admin: bool) -> Result<(), Error> {
    require_admin(pool).await?;
    sqlx::query("SELECT hydra_auth.hydra_set_admin($1, $2)")
        .bind(name)
        .bind(admin)
        .execute(pool)
        .await
        .map_err(refused)?;
    Ok(())
}

/// `false` also ends the user's sessions, so a disabled user is out at once: only once
/// disabling has committed, so the app can't sign in again in between.
pub async fn set_can_login(pool: &PgPool, name: &str, can_login: bool) -> Result<(), Error> {
    require_admin(pool).await?;
    sqlx::query("SELECT hydra_auth.hydra_set_can_login($1, $2)")
        .bind(name)
        .bind(can_login)
        .execute(pool)
        .await
        .map_err(refused)?;
    if !can_login {
        end_sessions(pool, name).await?;
    }
    Ok(())
}

/// Disconnect every session of `name`; returns how many ended. The app reconnects on its own
/// with the saved password, so to keep someone out, disable them or change their password.
pub async fn end_sessions(pool: &PgPool, name: &str) -> Result<usize, Error> {
    require_admin(pool).await?;
    let ended: i32 = sqlx::query_scalar("SELECT hydra_auth.hydra_end_sessions($1)")
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(refused)?;
    Ok(ended as usize)
}

/// Drops the role, then, once that has committed, ends the sessions it had. Their name stays
/// on revisions they saved.
pub async fn delete_user(pool: &PgPool, name: &str) -> Result<(), Error> {
    require_admin(pool).await?;
    let deleted: Oid = sqlx::query_scalar("SELECT hydra_auth.hydra_delete_user($1)")
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(refused)?;
    sqlx::query("SELECT hydra_auth.hydra_end_deleted_sessions($1)")
        .bind(deleted)
        .execute(pool)
        .await
        .map_err(refused)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::prelude::{BASE64_STANDARD, Engine as _};

    /// Verifiers PostgreSQL 18 stored for these passwords (`CREATE ROLE ... PASSWORD`, then
    /// pg_authid): same salt, same keys. The first one changes under SASLprep.
    #[test]
    fn verifiers_match_postgres() {
        for (password, stored) in [
            (
                "Gr\u{fc}\u{df}e\u{a0}\u{fb01}\u{ad}\u{2168}!",
                "SCRAM-SHA-256$4096:5M4V8v3comMDAbiCE5hX4Q==$JoWlWtZ1uTiSpTE47YEYIobZElYAi1DurL0NVaYmYwE=:AC1IqIMm5Kl4JLg+bUtq4ztWjNEd5NuKUVKhBAkN310=",
            ),
            (
                "pencil-pencil",
                "SCRAM-SHA-256$4096:C0exon3Ed0OrLynq3O3FIQ==$OmfwrcdvLcgsTjHxu5vLcw4o3Im3IUkxIMRD1HNShn4=:d+TmcEBMTc2MYfQ+VOrf3c4SQQJ9SBfumbK27qW1sVo=",
            ),
        ] {
            let salt = stored.split(['$', ':']).nth(2).unwrap();
            let salt = BASE64_STANDARD.decode(salt).unwrap();
            let prepared = stringprep::saslprep(password).unwrap();
            assert_eq!(scram(&prepared, &salt), stored, "{password}");
        }
        let fresh = verifier("pencil-pencil").unwrap();
        assert!(fresh.starts_with("SCRAM-SHA-256$4096:"), "{fresh}");
        assert_ne!(
            fresh,
            verifier("pencil-pencil").unwrap(),
            "a new salt each time"
        );
    }
}
