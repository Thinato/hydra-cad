//! Database access on a background thread. The UI stays synchronous and talks to it over channels.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use hydra_core::{Catalogue, CatalogueRef, Circuit, Proposal, ValveType};
use hydra_db::{
    Account, CircuitSummary, ImportReport, LoadedCatalogue, OpenedCircuit, PgConnectOptions,
    PgPool, ProposalRecord, RevisionSummary, User,
};
use hydra_mesh::Mesh;

/// What Postgres refusing the user name or password reads as.
pub const WRONG_LOGIN: &str = "Wrong user name or password.";
/// What Postgres refusing a role that may not log in reads as.
const CANT_SIGN_IN: &str = "Your account can't sign in (disabled by an administrator?).";

pub enum Request {
    LoadCatalogue,
    SaveValveType(ValveType),
    AttachDatasheet(CatalogueRef, Vec<u8>),
    Import(Catalogue),
    ListCircuits,
    OpenCircuit(String),
    /// Save a new revision from `base`, or create a new circuit when `id` is `None`.
    SaveCircuit {
        id: Option<String>,
        base: i32,
        document: Circuit,
        project: Option<String>,
        message: Option<String>,
    },
    ListRevisions(String),
    OpenRevision(String, i32),
    /// Restore an old revision as a new one, then reopen the circuit.
    RestoreRevision {
        id: String,
        number: i32,
        base: i32,
    },
    /// Open an asset with the system viewer, fetching it into the local cache first.
    OpenAsset {
        sha256: String,
        extension: &'static str,
    },
    /// Fetch a 3D model (glb) into the local cache and load it.
    LoadModel(String),
    ListProposals {
        pending_only: bool,
    },
    /// Approve a proposal as the reviewer edited it; the catalogue reloads after.
    ApproveProposal {
        id: String,
        proposal: Proposal,
    },
    RejectProposal {
        id: String,
        note: String,
    },
    /// Who is signed in; sent after every connect.
    Account,
    ListUsers,
    /// Change the signed-in user's own password.
    ChangePassword(String),
    /// An administrator's change to a user; the user list reloads after.
    ChangeUser(UserChange),
}

pub enum UserChange {
    Create {
        name: String,
        password: String,
        admin: bool,
    },
    SetPassword(String, String),
    SetAdmin(String, bool),
    SetCanLogin(String, bool),
    EndSessions(String),
    Delete(String),
}

/// Each request answers with its own outcome first; a catalogue reload follows any write.
pub enum Reply {
    Catalogue(LoadedCatalogue),
    Saved(CatalogueRef),
    /// Only a [`Request::SaveValveType`] fails this way, so the editor can tell its own error.
    SaveFailed(String),
    Attached(CatalogueRef),
    Imported(ImportReport),
    Circuits(Vec<CircuitSummary>),
    Opened(OpenedCircuit),
    CircuitSaved {
        id: String,
        revision: i32,
    },
    /// Someone saved the circuit since it was opened; nothing was written.
    Conflict {
        current: i32,
    },
    /// Saving a circuit failed, so the editor can stop waiting on the save.
    CircuitSaveFailed(String),
    /// Opening, listing or restoring circuits failed, so the editor can stop waiting on it.
    CircuitFailed(String),
    Revisions(Vec<RevisionSummary>),
    RevisionPreview(i32, Circuit),
    /// A [`Request::LoadModel`] answer, by hash.
    Model(String, Result<Arc<Mesh>, String>),
    Proposals(Vec<ProposalRecord>),
    ProposalApproved(CatalogueRef),
    ProposalRejected,
    /// Listing, approving or rejecting proposals failed, so the review tab can stop waiting.
    ProposalFailed(String),
    Account(Account),
    Users(Vec<User>),
    /// The own password changed; the app reconnects with the new one.
    PasswordChanged,
    /// What an administrator's change did, in plain words.
    UserChanged(String),
    /// An account or user request failed, so the Account tab can stop waiting.
    UserFailed(String),
    /// Postgres refused the user name or password; the answer to the initial catalogue load.
    WrongLogin,
    /// Connecting failed for another reason; the answer to the initial catalogue load.
    ConnectFailed(String),
    Error(String),
}

pub struct Db {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    /// What the worker connected with, to reconnect after a password change.
    options: Option<PgConnectOptions>,
}

impl Db {
    /// Start the worker and ask it for the catalogue and the account. Connection errors arrive
    /// as [`Reply::WrongLogin`] or [`Reply::ConnectFailed`].
    pub fn connect(options: PgConnectOptions, ctx: egui::Context) -> Db {
        let (requests, inbox) = channel();
        let (outbox, replies) = channel();
        // Names the app's sessions in the Account tab's user list.
        let options = options.application_name("Hydra");
        let kept = options.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(worker(options, inbox, outbox, ctx));
        });
        let db = Db {
            requests,
            replies,
            options: Some(kept),
        };
        db.send(Request::LoadCatalogue);
        db.send(Request::Account);
        db
    }

    /// A worker for the same database and user with a new password, for the app to swap in.
    pub fn with_password(&self, password: &str, ctx: egui::Context) -> Option<Db> {
        let options = self.options.clone()?.password(password);
        Some(Db::connect(options, ctx))
    }

    #[cfg(test)]
    pub fn new(requests: Sender<Request>, replies: Receiver<Reply>) -> Db {
        Db {
            requests,
            replies,
            options: None,
        }
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: PgConnectOptions) -> Db {
        self.options = Some(options);
        self
    }

    pub fn send(&self, request: Request) {
        // The worker only stops once its Db is retired.
        let _ = self.requests.send(request);
    }

    pub fn poll(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }

    /// Stop sending. The worker answers what it already has, then exits and disconnects the
    /// returned receiver.
    pub fn retire(self) -> Receiver<Reply> {
        self.replies
    }
}

async fn worker(
    options: PgConnectOptions,
    inbox: Receiver<Request>,
    outbox: Sender<Reply>,
    ctx: egui::Context,
) {
    let reply = |r: Reply| {
        let _ = outbox.send(r);
        ctx.request_repaint();
    };
    let pool = match connect(options).await {
        Ok(pool) => pool,
        Err(e) => {
            tracing::error!("database: {e}");
            let wrong_login = e.is_wrong_login();
            let e = match e {
                _ if wrong_login => WRONG_LOGIN.to_string(),
                hydra_db::Error::Sqlx(_) => format!("Can't reach the database: {e}"),
                _ => e.to_string(),
            };
            // Answer every request, starting with the initial catalogue load, so nothing in
            // the UI waits forever on a dead worker.
            while let Ok(request) = inbox.recv() {
                reply(match request {
                    Request::LoadCatalogue if wrong_login => Reply::WrongLogin,
                    Request::LoadCatalogue => Reply::ConnectFailed(e.clone()),
                    _ => failed(&request, e.clone()),
                });
            }
            return;
        }
    };
    // ponytail: one request at a time on a blocking channel; move to a tokio channel if the
    // UI ever needs to cancel or overlap requests.
    while let Ok(request) = inbox.recv() {
        for r in handle(&pool, request).await {
            reply(r);
        }
    }
}

async fn connect(options: PgConnectOptions) -> Result<PgPool, hydra_db::Error> {
    let pool = hydra_db::connect_with(options).await?;
    hydra_db::check_schema(&pool).await?;
    Ok(pool)
}

fn failed(request: &Request, e: String) -> Reply {
    match request {
        Request::SaveValveType(_) => Reply::SaveFailed(e),
        Request::SaveCircuit { .. } => Reply::CircuitSaveFailed(e),
        Request::LoadModel(sha) => Reply::Model(sha.clone(), Err(e)),
        Request::ListProposals { .. }
        | Request::ApproveProposal { .. }
        | Request::RejectProposal { .. } => Reply::ProposalFailed(e),
        Request::Account
        | Request::ListUsers
        | Request::ChangePassword(_)
        | Request::ChangeUser(_) => Reply::UserFailed(e),
        Request::RestoreRevision { .. }
        | Request::OpenCircuit(_)
        | Request::ListCircuits
        | Request::ListRevisions(_)
        | Request::OpenRevision(..) => Reply::CircuitFailed(e),
        _ => Reply::Error(e),
    }
}

/// The answer to a request that failed: a refused login starts the sign-in flow, a disabled
/// account gets one clear message, anything else goes to whoever asked.
fn failure(request: &Request, e: &hydra_db::Error) -> Reply {
    if e.is_wrong_login() {
        Reply::WrongLogin
    } else if e.is_login_refused() {
        failed(request, CANT_SIGN_IN.into())
    } else {
        failed(request, e.to_string())
    }
}

async fn handle(pool: &PgPool, request: Request) -> Vec<Reply> {
    use hydra_db as db;
    // Catalogue writes are followed by a reload; circuit requests answer on their own.
    let (outcome, reload_after) = match &request {
        Request::LoadCatalogue => return vec![reload(pool).await],
        Request::OpenAsset { sha256, extension } => {
            return match open_asset(pool, sha256, extension).await {
                Ok(()) => Vec::new(),
                Err(e) => vec![Reply::Error(e)],
            };
        }
        Request::LoadModel(sha) => {
            return vec![Reply::Model(sha.clone(), load_model(pool, sha).await)];
        }
        Request::ListProposals { pending_only } => (
            db::list_proposals(pool, *pending_only)
                .await
                .map(Reply::Proposals),
            false,
        ),
        Request::ApproveProposal { id, proposal } => (
            db::approve_proposal(pool, id, proposal)
                .await
                .map(Reply::ProposalApproved),
            true,
        ),
        Request::RejectProposal { id, note } => (
            db::reject_proposal(pool, id, note)
                .await
                .map(|()| Reply::ProposalRejected),
            false,
        ),
        Request::Account => (db::account(pool).await.map(Reply::Account), false),
        Request::ListUsers => return vec![list_users(pool).await],
        Request::ChangePassword(password) => (
            db::set_password(pool, None, password)
                .await
                .map(|()| Reply::PasswordChanged),
            false,
        ),
        Request::ChangeUser(change) => {
            return match change_user(pool, change).await {
                Ok(done) => vec![Reply::UserChanged(done), list_users(pool).await],
                Err(e) => vec![failure(&request, &e)],
            };
        }
        Request::SaveValveType(v) => (db::save_valve_type(pool, v).await.map(Reply::Saved), true),
        Request::AttachDatasheet(r, pdf) => (
            db::attach_datasheet(pool, r, pdf)
                .await
                .map(|_| Reply::Attached(r.clone())),
            true,
        ),
        Request::Import(c) => (
            db::import_catalogue(pool, c).await.map(Reply::Imported),
            true,
        ),
        Request::ListCircuits => (db::list_circuits(pool).await.map(Reply::Circuits), false),
        Request::OpenCircuit(id) => (db::open_circuit(pool, id).await.map(Reply::Opened), false),
        Request::SaveCircuit {
            id,
            base,
            document,
            project,
            message,
        } => {
            let saved = match id {
                Some(id) => db::save_revision(pool, id, *base, document, message.as_deref())
                    .await
                    .map(|revision| Reply::CircuitSaved {
                        id: id.clone(),
                        revision,
                    }),
                None => db::create_circuit(pool, document, project.as_deref(), message.as_deref())
                    .await
                    .map(|o| Reply::CircuitSaved {
                        id: o.id,
                        revision: o.revision,
                    }),
            };
            (saved, false)
        }
        Request::ListRevisions(id) => (
            db::list_revisions(pool, id).await.map(Reply::Revisions),
            false,
        ),
        Request::OpenRevision(id, n) => (
            db::open_revision(pool, id, *n)
                .await
                .map(|c| Reply::RevisionPreview(*n, c)),
            false,
        ),
        Request::RestoreRevision { id, number, base } => {
            let restored = match db::restore_revision(pool, id, *number, *base).await {
                Ok(_) => db::open_circuit(pool, id).await.map(Reply::Opened),
                Err(e) => Err(e),
            };
            (restored, false)
        }
    };
    match outcome {
        Ok(done) if reload_after => vec![done, reload(pool).await],
        Ok(done) => vec![done],
        Err(db::Error::Conflict { current }) => vec![Reply::Conflict { current }],
        Err(e) => vec![failure(&request, &e)],
    }
}

async fn reload(pool: &PgPool) -> Reply {
    match hydra_db::load_catalogue(pool).await {
        Ok(loaded) => Reply::Catalogue(loaded),
        Err(e) if e.is_wrong_login() => Reply::WrongLogin,
        Err(e) if e.is_login_refused() => Reply::Error(CANT_SIGN_IN.into()),
        Err(e) => Reply::Error(format!("Could not reload the catalogue: {e}")),
    }
}

async fn list_users(pool: &PgPool) -> Reply {
    match hydra_db::list_users(pool).await {
        Ok(users) => Reply::Users(users),
        Err(e) => failure(&Request::ListUsers, &e),
    }
}

/// Apply an administrator's change; returns what it did.
async fn change_user(pool: &PgPool, change: &UserChange) -> Result<String, hydra_db::Error> {
    use hydra_db as db;
    Ok(match change {
        UserChange::Create {
            name,
            password,
            admin,
        } => {
            db::create_user(pool, name, password, *admin).await?;
            format!("Added user {name}.")
        }
        UserChange::SetPassword(name, password) => {
            db::set_password(pool, Some(name), password).await?;
            format!("Changed the password of {name}.")
        }
        UserChange::SetAdmin(name, admin) => {
            db::set_admin(pool, name, *admin).await?;
            match admin {
                true => format!("{name} is now an administrator."),
                false => format!("{name} is no longer an administrator."),
            }
        }
        UserChange::SetCanLogin(name, can_login) => {
            db::set_can_login(pool, name, *can_login).await?;
            match can_login {
                true => format!("{name} can sign in again."),
                false => format!("Disabled {name} and ended their sessions."),
            }
        }
        UserChange::EndSessions(name) => {
            let n = db::end_sessions(pool, name).await?;
            format!("Ended {n} session(s) of {name}.")
        }
        UserChange::Delete(name) => {
            db::delete_user(pool, name).await?;
            format!("Deleted user {name}.")
        }
    })
}

/// Fetch an asset into the local cache if needed, then open it with the system viewer.
async fn open_asset(pool: &PgPool, sha256: &str, extension: &str) -> Result<(), String> {
    let (path, _) = fetch_asset(pool, sha256, extension).await?;
    open_with_system(&path).map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Fetch a model into the local cache if needed, then load it off the UI thread.
async fn load_model(pool: &PgPool, sha256: &str) -> Result<Arc<Mesh>, String> {
    let (_, bytes) = fetch_asset(pool, sha256, "glb").await?;
    hydra_mesh::load_glb(&bytes).map(Arc::new)
}

/// An asset's cache path and bytes, from the local cache or else from the database.
async fn fetch_asset(
    pool: &PgPool,
    sha256: &str,
    extension: &str,
) -> Result<(PathBuf, Vec<u8>), String> {
    // The hash names a file on disk, so only ever use one that is exactly a sha256.
    if !is_sha256_hex(sha256) {
        return Err(format!(
            "refusing to open asset with malformed hash {sha256:?}"
        ));
    }
    let path = cache_dir().join(format!("{sha256}.{extension}"));
    // Re-check cached files too: one could be truncated or from an older build.
    if let Ok(bytes) = std::fs::read(&path)
        && hydra_db::sha256_hex(&bytes) == sha256
    {
        return Ok((path, bytes));
    }
    {
        let bytes = hydra_db::asset_bytes(pool, sha256)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("asset {sha256} is not in the database"))?;
        if hydra_db::sha256_hex(&bytes) != sha256 {
            return Err(format!("asset {sha256} is corrupt in the database"));
        }
        // Write then rename, so a crash never leaves a truncated file that looks cached.
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(cache_dir())?;
            let partial = path.with_extension("partial");
            std::fs::write(&partial, &bytes)?;
            std::fs::rename(&partial, &path)
        };
        // The bytes are good either way; a cache that can't be written only costs a re-fetch.
        if let Err(e) = write() {
            tracing::warn!("could not cache {}: {e}", path.display());
        }
        Ok((path, bytes))
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Per-machine asset cache. Files are named by content hash, so they never go stale.
fn cache_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("hydra")
        .join("assets")
}

fn open_with_system(path: &Path) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Not `cmd /C start`: cmd would interpret characters like & in the path.
        std::process::Command::new("explorer.exe")
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(path).spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Wait for exactly `n` replies, failing the test after a timeout.
    fn take(db: &Db, n: usize) -> Vec<Reply> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut replies = Vec::new();
        while replies.len() < n {
            replies.extend(db.poll());
            assert!(
                Instant::now() < deadline,
                "the database worker did not reply in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(replies.len(), n, "unexpected extra replies");
        replies
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with the fixture catalogue imported)"]
    fn worker_lists_approves_and_rejects_proposals() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let proposal = |key: String| -> Proposal {
            serde_json::from_value(serde_json::json!({
                "valve_type": {
                    "key": key, "version": 0, "manufacturer": "demo", "series": "WORKER-TEST",
                    "code": "1", "symbol": "sym-check@1"
                }
            }))
            .unwrap()
        };
        let (good, bad) = (
            proposal(format!("vt-proposal-test-{nanos}")),
            proposal(format!("vt-proposal-test-{nanos}-b")),
        );
        let ids = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let pool = hydra_db::connect(&url).await.unwrap();
            [
                hydra_db::submit_proposal(&pool, &good).await.unwrap(),
                hydra_db::submit_proposal(&pool, &bad).await.unwrap(),
            ]
        });

        let db = Db::connect(url.parse().unwrap(), egui::Context::default());
        take(&db, 2); // the catalogue and the account
        db.send(Request::ListProposals { pending_only: true });
        let [Reply::Proposals(list)] = &take(&db, 1)[..] else {
            panic!("expected the proposals")
        };
        assert!(ids.iter().all(|id| list.iter().any(|p| &p.id == id)));

        db.send(Request::ApproveProposal {
            id: ids[0].clone(),
            proposal: good.clone(),
        });
        let [Reply::ProposalApproved(r), Reply::Catalogue(after)] = &take(&db, 2)[..] else {
            panic!("expected the approval, then a reload")
        };
        assert_eq!(*r, good.target());
        assert!(after.catalogue.valve_type(r).is_some());

        db.send(Request::RejectProposal {
            id: ids[1].clone(),
            note: "wrong spool".into(),
        });
        assert!(matches!(&take(&db, 1)[..], [Reply::ProposalRejected]));
        // A second review of the same proposal fails into the review tab.
        db.send(Request::RejectProposal {
            id: ids[1].clone(),
            note: "again".into(),
        });
        let [Reply::ProposalFailed(e)] = &take(&db, 1)[..] else {
            panic!("expected a failure")
        };
        assert!(e.contains("already rejected"), "{e}");
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with the fixture catalogue imported)"]
    fn worker_fetches_and_loads_models() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let glb = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/models/demo-block.glb"
        ))
        .unwrap();
        let block = hydra_db::ModelOwner::Block(CatalogueRef::new("blk-demo", 1));
        let sha = tokio::runtime::Runtime::new().unwrap().block_on(async {
            let pool = hydra_db::connect(&url).await.unwrap();
            hydra_db::attach_model(&pool, &block, &glb, None)
                .await
                .unwrap()
        });

        let db = Db::connect(url.parse().unwrap(), egui::Context::default());
        let [Reply::Catalogue(loaded), Reply::Account(_)] = &take(&db, 2)[..] else {
            panic!("expected the catalogue")
        };
        assert_eq!(
            loaded.block_models.get(&CatalogueRef::new("blk-demo", 1)),
            Some(&sha)
        );
        // Twice: once from the database, once from the local cache.
        let cached = cache_dir().join(format!("{sha}.glb"));
        let _ = std::fs::remove_file(&cached);
        for _ in 0..2 {
            db.send(Request::LoadModel(sha.clone()));
            let [Reply::Model(got, Ok(mesh))] = &take(&db, 1)[..] else {
                panic!("expected the model")
            };
            assert_eq!(got, &sha);
            let (lo, hi) = mesh.bounds();
            assert!((hi[0] - lo[0] - 130.0).abs() < 0.5, "{lo:?}..{hi:?}");
            assert!(cached.exists(), "the first load caches the model");
        }
        db.send(Request::LoadModel("0".repeat(64)));
        let [Reply::Model(_, Err(e))] = &take(&db, 1)[..] else {
            panic!("expected a failure")
        };
        assert!(e.contains("not in the database"), "{e}");
        db.send(Request::LoadModel("../etc/passwd".into()));
        let [Reply::Model(_, Err(e))] = &take(&db, 1)[..] else {
            panic!("expected a failure")
        };
        assert!(e.contains("malformed hash"), "{e}");
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with the fixture catalogue imported)"]
    fn worker_loads_saves_and_attaches() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let db = Db::connect(url.parse().unwrap(), egui::Context::default());
        let [Reply::Catalogue(loaded), Reply::Account(_)] = &take(&db, 2)[..] else {
            panic!("expected the catalogue")
        };
        let template = loaded.catalogue.valve_types[0].clone();

        // A key no earlier run used; base version 0 means "new key".
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let draft = ValveType {
            key: format!("vt-worker-test-{nanos}"),
            version: 0,
            series: "WORKER-TEST".into(),
            ..template
        };
        db.send(Request::SaveValveType(draft.clone()));
        let [Reply::Saved(r), Reply::Catalogue(after)] = &take(&db, 2)[..] else {
            panic!("expected Saved, then a reload")
        };
        assert_eq!(*r, CatalogueRef::new(&draft.key, 1));
        assert!(after.catalogue.valve_type(r).is_some());

        // Saving from the same stale base again must fail into the editor, not add @2.
        db.send(Request::SaveValveType(draft.clone()));
        let [Reply::SaveFailed(e)] = &take(&db, 1)[..] else {
            panic!("expected SaveFailed")
        };
        assert!(e.contains("already exists"), "{e}");

        db.send(Request::AttachDatasheet(
            r.clone(),
            b"%PDF-1.4 worker test".to_vec(),
        ));
        let [Reply::Attached(a), Reply::Catalogue(after)] = &take(&db, 2)[..] else {
            panic!("expected Attached, then a reload")
        };
        assert_eq!(a, r);
        assert!(after.datasheets.contains_key(r));

        db.send(Request::AttachDatasheet(r.clone(), b"not a pdf".to_vec()));
        let [Reply::Error(e)] = &take(&db, 1)[..] else {
            panic!("expected an error")
        };
        assert!(e.contains("not a PDF"), "{e}");
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with the fixture catalogue imported)"]
    fn worker_saves_conflicts_and_restores_circuits() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let db = Db::connect(url.parse().unwrap(), egui::Context::default());
        take(&db, 2); // the catalogue and the account
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/circuits/demo-manifold.json"
        );
        let mut doc = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let save = |id: Option<String>, base: i32, document: Circuit| Request::SaveCircuit {
            id,
            base,
            document,
            project: Some("worker test".into()),
            message: Some("from the worker test".into()),
        };

        db.send(save(None, 0, doc.clone()));
        let [Reply::CircuitSaved { id, revision: 1 }] = &take(&db, 1)[..] else {
            panic!("expected revision 1 of a new circuit")
        };
        let id = id.clone();
        doc.name = "Demo manifold, edited".into();
        db.send(save(Some(id.clone()), 1, doc.clone()));
        assert!(matches!(
            &take(&db, 1)[..],
            [Reply::CircuitSaved { revision: 2, .. }]
        ));

        // A second editor still on revision 1 is told about revision 2.
        db.send(save(Some(id.clone()), 1, doc.clone()));
        assert!(matches!(
            &take(&db, 1)[..],
            [Reply::Conflict { current: 2 }]
        ));

        db.send(Request::ListRevisions(id.clone()));
        let [Reply::Revisions(revs)] = &take(&db, 1)[..] else {
            panic!("expected revisions")
        };
        assert_eq!(revs.iter().map(|r| r.number).collect::<Vec<_>>(), [2, 1]);

        db.send(Request::OpenRevision(id.clone(), 1));
        assert!(
            matches!(&take(&db, 1)[..], [Reply::RevisionPreview(1, c)] if c.name.starts_with("Demo manifold ("))
        );

        // Restoring 1 creates revision 3 with 1's content and reopens the circuit.
        db.send(Request::RestoreRevision {
            id: id.clone(),
            number: 1,
            base: 2,
        });
        let [Reply::Opened(opened)] = &take(&db, 1)[..] else {
            panic!("expected the reopened circuit")
        };
        assert_eq!(opened.revision, 3);
        assert!(opened.document.name.starts_with("Demo manifold ("));

        // A stale restore is a conflict that the editor hears about.
        db.send(Request::RestoreRevision {
            id: id.clone(),
            number: 1,
            base: 2,
        });
        assert!(matches!(
            &take(&db, 1)[..],
            [Reply::Conflict { current: 3 }]
        ));
        db.send(Request::OpenCircuit("not-a-uuid".into()));
        assert!(
            matches!(&take(&db, 1)[..], [Reply::CircuitFailed(e)] if e.contains("unknown circuit"))
        );
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with password authentication)"]
    fn worker_reports_a_wrong_login() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        let db = Db::connect(
            options.password("not-the-password"),
            egui::Context::default(),
        );
        assert!(matches!(
            &take(&db, 2)[..],
            [Reply::WrongLogin, Reply::UserFailed(_)]
        ));
        db.send(Request::ListCircuits);
        assert!(matches!(&take(&db, 1)[..], [Reply::CircuitFailed(e)] if e == WRONG_LOGIN));
    }

    #[test]
    #[ignore = "needs DATABASE_URL (Postgres with the fixture catalogue imported)"]
    fn worker_reports_the_account_and_refuses_non_admins() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let engineer = format!("hydra_t_{nanos}");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pool = rt.block_on(hydra_db::connect(&url)).unwrap();
        // A superuser is an administrator.
        let user = options.get_username().to_string();
        let db = Db::connect(options.clone(), egui::Context::default());
        let [Reply::Catalogue(_), Reply::Account(account)] = &take(&db, 2)[..] else {
            panic!("expected the catalogue and the account")
        };
        assert_eq!((account.user.as_str(), account.admin), (&user[..], true));

        // A plain engineer is not, and Postgres refuses what only administrators may do.
        rt.block_on(hydra_db::create_user(
            &pool,
            &engineer,
            "engineer-password-1",
            false,
        ))
        .unwrap();
        let db = Db::connect(
            options.username(&engineer).password("engineer-password-1"),
            egui::Context::default(),
        );
        let replies = take(&db, 2);
        let [Reply::Catalogue(_), Reply::Account(account)] = &replies[..] else {
            panic!("expected the catalogue and the account")
        };
        assert_eq!(
            (account.user.as_str(), account.admin),
            (&engineer[..], false)
        );
        db.send(Request::ListUsers);
        let [Reply::UserFailed(e)] = &take(&db, 1)[..] else {
            panic!("expected a refusal")
        };
        assert!(e.contains("only an administrator"), "{e}");
        db.send(Request::ChangeUser(UserChange::Delete("nobody".into())));
        let [Reply::UserFailed(e)] = &take(&db, 1)[..] else {
            panic!("expected a refusal")
        };
        assert!(e.contains("only an administrator"), "{e}");
        rt.block_on(hydra_db::delete_user(&pool, &engineer))
            .unwrap();
    }

    #[test]
    fn only_real_hashes_name_cache_files() {
        assert!(is_sha256_hex(&"ab12".repeat(16)));
        for bad in [
            "",
            "../../etc/passwd",
            &"AB12".repeat(16),
            &"ab12".repeat(15),
            "x&calc&rem",
        ] {
            assert!(!is_sha256_hex(bad), "{bad}");
        }
    }

    #[test]
    fn unreachable_database_reports_an_error() {
        let db = Db::connect(
            "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
            egui::Context::default(),
        );
        let [Reply::ConnectFailed(e), Reply::UserFailed(_)] = &take(&db, 2)[..] else {
            panic!("expected a connection failure")
        };
        assert!(e.starts_with("Can't reach the database"), "{e}");
        // Later requests still get an answer, and a save reports to the editor.
        db.send(Request::SaveValveType(ValveType {
            key: "k".into(),
            version: 0,
            manufacturer: "m".into(),
            series: "s".into(),
            code: "c".into(),
            symbol: None,
            mounting: None,
            params: serde_json::Value::Null,
        }));
        assert!(
            matches!(&take(&db, 1)[..], [Reply::SaveFailed(e)] if e.starts_with("Can't reach"))
        );
    }
}
