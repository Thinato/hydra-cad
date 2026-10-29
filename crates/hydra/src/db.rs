//! Database access on a background thread. The UI stays synchronous and talks to it over channels.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use hydra_core::{Catalogue, CatalogueRef, ValveType};
use hydra_db::{ImportReport, LoadedCatalogue, PgPool};

pub enum Request {
    LoadCatalogue,
    SaveValveType(ValveType),
    AttachDatasheet(CatalogueRef, Vec<u8>),
    Import(Catalogue),
    /// Open an asset with the system viewer, fetching it into the local cache first.
    OpenAsset {
        sha256: String,
        extension: &'static str,
    },
}

/// Each request answers with its own outcome first; a catalogue reload follows any write.
pub enum Reply {
    Catalogue(LoadedCatalogue),
    Saved(CatalogueRef),
    /// Only a [`Request::SaveValveType`] fails this way, so the editor can tell its own error.
    SaveFailed(String),
    Attached(CatalogueRef),
    Imported(ImportReport),
    Error(String),
}

pub struct Db {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
}

impl Db {
    /// Start the worker and ask it for the catalogue. Connection errors arrive as [`Reply::Error`].
    pub fn connect(url: String, ctx: egui::Context) -> Db {
        let (requests, inbox) = channel();
        let (outbox, replies) = channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(worker(url, inbox, outbox, ctx));
        });
        let db = Db { requests, replies };
        db.send(Request::LoadCatalogue);
        db
    }

    pub fn send(&self, request: Request) {
        // The worker only stops when the app does.
        let _ = self.requests.send(request);
    }

    pub fn poll(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }
}

async fn worker(url: String, inbox: Receiver<Request>, outbox: Sender<Reply>, ctx: egui::Context) {
    let reply = |r: Reply| {
        let _ = outbox.send(r);
        ctx.request_repaint();
    };
    let pool = match connect(&url).await {
        Ok(pool) => pool,
        Err(e) => {
            tracing::error!("database: {e}");
            // Answer every request, starting with the initial catalogue load, so nothing in
            // the UI waits forever on a dead worker.
            while let Ok(request) = inbox.recv() {
                reply(failed(&request, e.clone()));
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

async fn connect(url: &str) -> Result<PgPool, String> {
    let pool = hydra_db::connect(url)
        .await
        .map_err(|e| format!("Can't reach the database: {e}"))?;
    hydra_db::check_schema(&pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(pool)
}

fn failed(request: &Request, e: String) -> Reply {
    match request {
        Request::SaveValveType(_) => Reply::SaveFailed(e),
        _ => Reply::Error(e),
    }
}

async fn handle(pool: &PgPool, request: Request) -> Vec<Reply> {
    let outcome = match &request {
        Request::LoadCatalogue => return vec![reload(pool).await],
        Request::SaveValveType(v) => hydra_db::save_valve_type(pool, v).await.map(Reply::Saved),
        Request::AttachDatasheet(r, pdf) => hydra_db::attach_datasheet(pool, r, pdf)
            .await
            .map(|_| Reply::Attached(r.clone())),
        Request::Import(c) => hydra_db::import_catalogue(pool, c)
            .await
            .map(Reply::Imported),
        Request::OpenAsset { sha256, extension } => {
            return match open_asset(pool, sha256, extension).await {
                Ok(()) => Vec::new(),
                Err(e) => vec![Reply::Error(e)],
            };
        }
    };
    match outcome {
        Ok(done) => vec![done, reload(pool).await],
        Err(e) => vec![failed(&request, e.to_string())],
    }
}

async fn reload(pool: &PgPool) -> Reply {
    match hydra_db::load_catalogue(pool).await {
        Ok(loaded) => Reply::Catalogue(loaded),
        Err(e) => Reply::Error(format!("Could not reload the catalogue: {e}")),
    }
}

/// Fetch an asset into the local cache if needed, then open it with the system viewer.
async fn open_asset(pool: &PgPool, sha256: &str, extension: &str) -> Result<(), String> {
    // The hash names a file on disk, so only ever use one that is exactly a sha256.
    if !is_sha256_hex(sha256) {
        return Err(format!(
            "refusing to open asset with malformed hash {sha256:?}"
        ));
    }
    let path = cache_dir().join(format!("{sha256}.{extension}"));
    // Re-check cached files too: one could be truncated or from an older build.
    let cached_ok = std::fs::read(&path).is_ok_and(|b| hydra_db::sha256_hex(&b) == sha256);
    if !cached_ok {
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
        write().map_err(|e| format!("could not cache {}: {e}", path.display()))?;
    }
    open_with_system(&path).map_err(|e| format!("could not open {}: {e}", path.display()))
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
    fn worker_loads_saves_and_attaches() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let db = Db::connect(url, egui::Context::default());
        let [Reply::Catalogue(loaded)] = &take(&db, 1)[..] else {
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
            "postgres://nobody@127.0.0.1:1/none".into(),
            egui::Context::default(),
        );
        let [Reply::Error(e)] = &take(&db, 1)[..] else {
            panic!("expected an error")
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
