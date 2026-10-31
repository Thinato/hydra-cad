// No console window behind the installed Windows app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod catalogue;
mod config;
mod db;
mod editor;
mod models;
mod proposals;
mod schematic;
mod signin;
mod update;
mod viewer3d;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use hydra_core::{Catalogue, Circuit};

use catalogue::{CataloguePanel, Shared};
use db::{Db, Reply, Request};
use signin::SignIn;
use update::Update;

// ponytail: dev fixture paths baked in at compile time, used only where they exist, so an
// installed app starts on an empty circuit.
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const DEMO_CIRCUIT: &str = "fixtures/circuits/demo-manifold.json";
const CATALOGUE: &str = "fixtures/catalogue.json";
/// Full Postgres URL with credentials, for dev and CI: overrides hydra.ini and skips sign-in.
const DATABASE_URL_VAR: &str = "HYDRA_DATABASE_URL";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Schematic,
    Catalogue,
    Proposals,
    Account,
    View3d,
}

struct Panels {
    shared: Shared,
    schematic: editor::SchematicEditor,
    catalogue: CataloguePanel,
    models: models::ModelPanel,
    proposals: proposals::ProposalsPanel,
    account: account::AccountPanel,
    /// Set when the database comes from hydra.ini, so each user signs in.
    signin: Option<SignIn>,
}

impl TabViewer for Panels {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab as u8)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Schematic => "Schematic".into(),
            Tab::Catalogue => "Catalogue".into(),
            Tab::Proposals => "Proposals".into(),
            Tab::Account => "Account".into(),
            Tab::View3d => "3D".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Schematic => self.schematic.ui(ui, &mut self.shared),
            Tab::Catalogue => self.catalogue.ui(ui, &mut self.shared),
            Tab::Proposals => self.proposals.ui(ui, &mut self.shared),
            Tab::Account => self.account.ui(ui, &mut self.shared, self.signin.as_mut()),
            Tab::View3d => self.models.ui(ui, &self.shared, &mut self.schematic),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }
}

impl Panels {
    /// Apply what the database worker sent since the last frame, and what retired workers of
    /// earlier sign-ins still answer, so no panel waits forever on a request.
    fn poll_db(&mut self, ctx: &egui::Context) {
        let mut replies: Vec<_> = self
            .shared
            .db
            .iter()
            .flat_map(Db::poll)
            .map(|r| (r, false))
            .collect();
        self.shared.retired.retain(|r| {
            loop {
                match r.try_recv() {
                    Ok(reply) => replies.push((reply, true)),
                    Err(TryRecvError::Empty) => break true,
                    Err(TryRecvError::Disconnected) => break false,
                }
            }
        });
        for (reply, retired) in replies {
            match reply {
                // Only the current sign-in's connection outcome counts. An old worker's
                // catalogue follows a write it finished: reload through the current one.
                Reply::WrongLogin | Reply::ConnectFailed(_) if retired => {}
                // An old sign-in's account and users must not show under the current one.
                Reply::Account(_) | Reply::Users(_) | Reply::UserFailed(_) if retired => {}
                Reply::PasswordChanged if retired => {
                    self.shared.notice = Some("Your password was changed.".into())
                }
                Reply::Catalogue(_) if retired => {
                    if let Some(db) = &self.shared.db {
                        db.send(Request::LoadCatalogue);
                    }
                }
                Reply::Catalogue(loaded) => {
                    self.shared.catalogue = loaded.catalogue;
                    self.shared.datasheets = loaded.datasheets;
                    self.shared.valve_models = loaded.valve_models;
                    self.shared.block_models = loaded.block_models;
                    self.models.retry_failed();
                    self.shared.error = None;
                    if let Some(s) = &mut self.signin {
                        s.connected();
                    }
                }
                Reply::Saved(r) => {
                    self.shared.notice = Some(format!("Saved {r}."));
                    self.catalogue.saved(r);
                }
                Reply::Attached(r) => {
                    self.shared.notice = Some(format!("Datasheet attached to {r}."))
                }
                Reply::Imported(r) => {
                    self.shared.notice = Some(format!(
                        "Imported {} manufacturers, {} symbols, {} valve types, {} parts and {} blocks ({} rows unchanged).",
                        r.manufacturers_added,
                        r.symbol_defs_added,
                        r.valve_types_added,
                        r.parts_added,
                        r.blocks_added,
                        r.unchanged
                    ))
                }
                Reply::SaveFailed(e) => {
                    if !self.catalogue.save_failed(&e) {
                        self.shared.error = Some(e);
                    }
                }
                Reply::Circuits(list) => self.schematic.circuits(list),
                Reply::Opened(opened) => self.schematic.opened(opened),
                Reply::CircuitSaved { id, revision } => self.schematic.circuit_saved(id, revision),
                Reply::Conflict { current } => self.schematic.conflict(current),
                Reply::CircuitSaveFailed(e) => self.schematic.save_failed(e),
                Reply::CircuitFailed(e) => self.schematic.circuit_failed(e),
                Reply::Revisions(list) => self.schematic.revisions(list),
                Reply::RevisionPreview(n, c) => {
                    self.schematic
                        .revision_preview(n, &c, &self.shared.catalogue)
                }
                Reply::Model(sha, mesh) => self.models.loaded(sha, mesh),
                Reply::Proposals(list) => self.proposals.listed(list),
                Reply::ProposalApproved(r) => {
                    self.shared.notice = Some(format!("Approved: {r} is in the catalogue."));
                    self.proposals.reviewed(&self.shared);
                }
                Reply::ProposalRejected => {
                    self.shared.notice = Some("Proposal rejected.".into());
                    self.proposals.reviewed(&self.shared);
                }
                Reply::ProposalFailed(e) => self.proposals.failed(e),
                Reply::Account(a) => self.account.signed_in(a, &self.shared),
                Reply::Users(list) => self.account.users(list),
                Reply::UserChanged(done) => self.shared.notice = Some(done),
                Reply::UserFailed(e) => self.account.failed(e),
                Reply::PasswordChanged => {
                    let password = self.account.password_changed();
                    let notice = match &mut self.signin {
                        Some(s) => {
                            let lost = password
                                .is_some_and(|p| s.password_changed(p, &mut self.shared, ctx));
                            match lost {
                                true => "Password changed. Couldn't remember the new password on this computer."
                                    .into(),
                                false => "Password changed.".into(),
                            }
                        }
                        None => {
                            // New connections need the new password, so swap in a new worker.
                            let new = password.and_then(|p| {
                                self.shared.db.as_ref()?.with_password(&p, ctx.clone())
                            });
                            if let Some(new) = new
                                && let Some(old) = self.shared.db.replace(new)
                            {
                                self.shared.retired.push(old.retire());
                            }
                            format!(
                                "Password changed. {DATABASE_URL_VAR} still holds the old one; update it before you start Hydra again."
                            )
                        }
                    };
                    self.shared.notice = Some(notice);
                }
                Reply::WrongLogin => match &mut self.signin {
                    Some(s) => s.refused(&mut self.shared),
                    None => {
                        self.shared.error = Some(db::WRONG_LOGIN.into());
                        self.account.failed(db::WRONG_LOGIN.into());
                    }
                },
                Reply::ConnectFailed(e) => {
                    self.shared.error = Some(e);
                    if let Some(s) = &mut self.signin {
                        s.connect_failed();
                    }
                }
                Reply::Error(e) => self.shared.error = Some(e),
            }
        }
        if self.shared.db.is_none() {
            self.account.forget();
        }
    }
}

struct HydraApp {
    dock: DockState<Tab>,
    panels: Panels,
    updates: Option<Receiver<Update>>,
    update: Option<Update>,
}

impl eframe::App for HydraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.panels.poll_db(ui.ctx());
        if let Some(u) = self.updates.as_ref().and_then(|r| r.try_recv().ok()) {
            self.update = Some(u);
        }
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.panels.schematic.may_close()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let panels = &mut self.panels;
        panels
            .catalogue
            .handle_dropped_files(ui.ctx(), &mut panels.shared);
        // Status line above the tabs, so database errors show whichever tab is open.
        let shared = &panels.shared;
        if shared.error.is_some()
            || shared.notice.is_some()
            || panels.signin.is_some()
            || self.update.is_some()
        {
            ui.horizontal_wrapped(|ui| {
                if let Some(s) = &mut panels.signin {
                    s.status(ui, &mut panels.shared);
                }
                if let Some(e) = &panels.shared.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if let Some(n) = &panels.shared.notice {
                    ui.weak(n);
                }
                if let Some(u) = &self.update {
                    let link = ui.hyperlink_to(format!("Download Hydra {}", u.version), &u.url);
                    if let Some(notes) = &u.notes {
                        link.on_hover_text(notes);
                    }
                }
            });
        }
        DockArea::new(&mut self.dock)
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut self.panels);
        if let Some(s) = &mut self.panels.signin {
            s.ui(ui.ctx(), &mut self.panels.shared);
        }
    }
}

fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// Where release builds write their log.
fn log_path() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Hydra"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Logs/Hydra"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .map(|d| d.join("hydra"))
    };
    dir.map(|d| d.join("hydra.log"))
}

/// `hydra [CIRCUIT.json] [CATALOGUE.json]`; in a dev checkout they default to the demo fixtures.
///
/// The database is `HYDRA_DATABASE_URL`, else `database_url` from hydra.ini with a sign-in.
/// Without either the app runs offline and read-only on the catalogue file.
fn main() -> eframe::Result {
    // A release build has no console on Windows, so it logs to a file, emptied on each start.
    let log = (!cfg!(debug_assertions))
        .then(log_path)
        .flatten()
        .and_then(|path| {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::File::create(path).ok()
        });
    match log {
        Some(file) => tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .init(),
        None => tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init(),
    }
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let dev = |file: &str| Some(PathBuf::from(ROOT).join(file)).filter(|p| p.exists());
    let circuit_path = args.next().or_else(|| dev(DEMO_CIRCUIT));
    let catalogue_path = args.next().or_else(|| dev(CATALOGUE));
    let env_url = std::env::var(DATABASE_URL_VAR).ok();
    let (config, problems) = config::load();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: viewer3d::DEPTH_BUFFER_BITS,
        multisampling: viewer3d::MSAA_SAMPLES,
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Hydra",
        options,
        Box::new(move |cc| {
            let ctx = &cc.egui_ctx;
            let mut shared = Shared::new(Catalogue::default());
            let mut errors = problems;
            let mut signin = None;
            if let Some(url) = env_url {
                match url.parse() {
                    Ok(options) => shared.db = Some(Db::connect(options, ctx.clone())),
                    Err(e) => errors.push(format!("{DATABASE_URL_VAR} is not a Postgres URL: {e}")),
                }
            } else if let Some(url) = config.database_url {
                match SignIn::new(url) {
                    Ok(mut s) => {
                        s.start(&mut shared, ctx);
                        signin = Some(s);
                    }
                    Err(e) => errors.push(e),
                }
            } else if let Some(path) = catalogue_path {
                match read(&path).and_then(|t| Ok(serde_json::from_str::<Catalogue>(&t)?)) {
                    Ok(c) => {
                        shared.catalogue = c;
                        shared.notice = Some(format!(
                            "Offline: read-only catalogue from {}. Set database_url in {} or {DATABASE_URL_VAR} to use the shared database.",
                            path.display(),
                            config::FILE
                        ));
                    }
                    Err(e) => errors.push(format!("Could not load the catalogue: {e:#}")),
                }
            } else {
                let file = config::machine_path().unwrap_or_else(|| config::FILE.into());
                errors.push(format!(
                    "No database is set up. An administrator must set database_url in {}.",
                    file.display()
                ));
            }
            let updates = config.update_url.map(|url| update::check(url, ctx.clone()));
            // The file opens as a new, unsaved circuit; Save puts it in the database.
            let circuit = match &circuit_path {
                Some(path) => read(path)
                    .and_then(|t| Ok(Circuit::from_json(&t)?))
                    .unwrap_or_else(|e| {
                        tracing::error!("could not load circuit: {e:#}");
                        errors.push(format!("Could not load the circuit: {e:#}"));
                        editor::empty_circuit()
                    }),
                None => editor::empty_circuit(),
            };
            if !errors.is_empty() {
                shared.error = Some(errors.join(" "));
            }
            let schematic = editor::SchematicEditor::new(circuit, None);

            let mut dock = DockState::new(vec![
                Tab::Schematic,
                Tab::Catalogue,
                Tab::Proposals,
                Tab::Account,
            ]);
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.6, vec![Tab::View3d]);
            Ok(Box::new(HydraApp {
                dock,
                panels: Panels {
                    shared,
                    schematic,
                    catalogue: CataloguePanel::default(),
                    proposals: proposals::ProposalsPanel::default(),
                    account: account::AccountPanel::default(),
                    models: models::ModelPanel::new(
                        viewer3d::Viewer3d::new(cc)
                            .inspect_err(|e| tracing::error!("3D view: {e:#}"))
                            .ok(),
                    ),
                    signin,
                },
                updates,
                update: None,
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};

    use hydra_core::CatalogueRef;

    use super::*;

    #[test]
    fn replies_from_a_retired_worker_still_arrive() {
        let (requests, inbox) = channel();
        let (outbox, replies) = channel();
        let mut shared = Shared::new(Catalogue::default());
        shared.db = Some(Db::new(requests, replies));
        let mut panels = Panels {
            shared,
            schematic: editor::SchematicEditor::new(editor::empty_circuit(), None),
            catalogue: CataloguePanel::default(),
            models: models::ModelPanel::new(None),
            proposals: proposals::ProposalsPanel::default(),
            account: account::AccountPanel::default(),
            signin: None,
        };
        // Signing out retires the worker: it gets no more requests, so it exits once done.
        let db = panels.shared.db.take().unwrap();
        panels.shared.retired.push(db.retire());
        assert_eq!(inbox.try_recv().err(), Some(TryRecvError::Disconnected));

        // It finishes a save, then its failed initial load reports a wrong login.
        outbox
            .send(Reply::Saved(CatalogueRef::new("vt-a", 2)))
            .unwrap();
        panels.poll_db(&egui::Context::default());
        assert_eq!(panels.shared.notice.as_deref(), Some("Saved vt-a@2."));
        assert_eq!(panels.shared.retired.len(), 1);

        // Signed in again: the old worker's reload after its save goes through the new one.
        let (requests, new_inbox) = channel();
        let (_new_outbox, replies) = channel();
        panels.shared.db = Some(Db::new(requests, replies));
        let mut stale = hydra_db::LoadedCatalogue::default();
        stale
            .catalogue
            .manufacturers
            .push(hydra_core::Manufacturer {
                key: "stale".into(),
                name: "Stale".into(),
            });
        outbox.send(Reply::Catalogue(stale)).unwrap();
        panels.poll_db(&egui::Context::default());
        assert!(panels.shared.catalogue.manufacturers.is_empty());
        assert!(matches!(new_inbox.try_recv(), Ok(Request::LoadCatalogue)));
        outbox.send(Reply::WrongLogin).unwrap();
        drop(outbox);
        panels.poll_db(&egui::Context::default());
        assert_eq!(panels.shared.error, None);
        assert!(panels.shared.retired.is_empty());
    }

    #[test]
    fn a_retired_workers_account_and_users_are_ignored() {
        let ctx = egui::Context::default();
        let (requests, inbox) = channel();
        let (outbox, replies) = channel();
        let mut shared = Shared::new(Catalogue::default());
        shared.db = Some(Db::new(requests, replies));
        let mut panels = Panels {
            shared,
            schematic: editor::SchematicEditor::new(editor::empty_circuit(), None),
            catalogue: CataloguePanel::default(),
            models: models::ModelPanel::new(None),
            proposals: proposals::ProposalsPanel::default(),
            account: account::AccountPanel::default(),
            signin: None,
        };
        let boss = hydra_db::Account {
            user: "boss".into(),
            admin: true,
        };
        let users = || {
            vec![hydra_db::User {
                name: "boss".into(),
                admin: true,
                can_login: true,
                sessions: Vec::new(),
            }]
        };
        // The current worker's admin account loads the users.
        outbox.send(Reply::Account(boss.clone())).unwrap();
        panels.poll_db(&ctx);
        assert!(matches!(inbox.try_recv(), Ok(Request::ListUsers)));
        outbox.send(Reply::Users(users())).unwrap();
        panels.poll_db(&ctx);
        assert_eq!(panels.account.shown().1.map(Vec::len), Some(1));

        // Signed out: nothing of that sign-in stays, and its late replies change nothing.
        let db = panels.shared.db.take().unwrap();
        panels.shared.retired.push(db.retire());
        panels.poll_db(&ctx);
        assert_eq!(panels.account.shown(), (None, None));
        outbox.send(Reply::Account(boss)).unwrap();
        outbox.send(Reply::Users(users())).unwrap();
        outbox.send(Reply::UserFailed("late".into())).unwrap();
        outbox
            .send(Reply::UserChanged("Deleted user x.".into()))
            .unwrap();
        // Someone else signs in before the old worker's replies are polled.
        let (requests, new_inbox) = channel();
        panels.shared.db = Some(Db::new(requests, channel().1));
        panels.poll_db(&ctx);
        assert_eq!(panels.account.shown(), (None, None));
        assert!(
            new_inbox.try_recv().is_err(),
            "no user list for the new sign-in"
        );
        assert_eq!(panels.shared.notice.as_deref(), Some("Deleted user x."));
    }

    fn panels_with_db(signin: Option<SignIn>) -> (Panels, Receiver<Request>, Sender<Reply>) {
        let (requests, inbox) = channel();
        let (outbox, replies) = channel();
        let mut shared = Shared::new(Catalogue::default());
        shared.db = Some(Db::new(requests, replies));
        let panels = Panels {
            shared,
            schematic: editor::SchematicEditor::new(editor::empty_circuit(), None),
            catalogue: CataloguePanel::default(),
            models: models::ModelPanel::new(None),
            proposals: proposals::ProposalsPanel::default(),
            account: account::AccountPanel::default(),
            signin,
        };
        (panels, inbox, outbox)
    }

    #[test]
    fn a_login_refused_mid_session_signs_out_or_shows_the_error() {
        let ctx = egui::Context::default();
        // Through hydra.ini: the sign-in window comes back and the old worker is retired.
        let (mut panels, _inbox, outbox) = panels_with_db(Some(SignIn::test("anna", "pw")));
        outbox.send(Reply::WrongLogin).unwrap();
        panels.poll_db(&ctx);
        assert!(panels.shared.db.is_none());
        assert_eq!(panels.shared.retired.len(), 1);
        assert_eq!(panels.signin.as_ref().unwrap().user(), None);

        // Through HYDRA_DATABASE_URL: the error shows and the Account tab stops waiting.
        let (mut panels, _inbox, outbox) = panels_with_db(None);
        panels.account.failed(String::new());
        outbox.send(Reply::WrongLogin).unwrap();
        panels.poll_db(&ctx);
        assert_eq!(panels.shared.error.as_deref(), Some(db::WRONG_LOGIN));
        assert!(panels.shared.db.is_some());
    }

    #[test]
    fn a_new_password_through_the_environment_reconnects() {
        let ctx = egui::Context::default();
        let (mut panels, old_inbox, outbox) = panels_with_db(None);
        let options = "postgres://anna:old@127.0.0.1:1/none".parse().unwrap();
        panels.shared.db = panels.shared.db.take().map(|d| d.with_options(options));
        panels.account.set_changing("new-password-1");
        outbox.send(Reply::PasswordChanged).unwrap();
        panels.poll_db(&ctx);
        assert!(panels.shared.db.is_some(), "a new worker took over");
        assert_eq!(panels.shared.retired.len(), 1, "the old worker is retired");
        assert_eq!(old_inbox.try_recv().err(), Some(TryRecvError::Disconnected));
        assert!(
            panels
                .shared
                .notice
                .unwrap()
                .contains("still holds the old one")
        );
    }
}
