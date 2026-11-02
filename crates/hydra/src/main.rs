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
mod theme;
mod update;
mod viewer3d;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, RichText};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use hydra_core::{Catalogue, Circuit};

use catalogue::{CataloguePanel, Shared};
use db::{Db, Reply, Request};
use signin::SignIn;
use theme::Mark;
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
    /// Where egui_dock drew each tab this frame, for the active tabs' accent edge.
    tab_rects: Vec<(Tab, egui::Rect)>,
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
            // The waiting count the Proposals tab loaded, never a guess: none before it has.
            Tab::Proposals => match self.shared.db.as_ref().and(self.proposals.waiting()) {
                Some(n) => theme::counted("Proposals", n),
                None => "Proposals".into(),
            },
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

    // ponytail: no floating windows, so the accent edge is only painted on the main surface's
    // layer; record each tab's layer if windows are ever wanted.
    fn allowed_in_windows(&self, _tab: &mut Tab) -> bool {
        false
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        // Clipped to what shows of a strip that scrolls.
        self.tab_rects.push((*tab, response.interact_rect));
    }
}

/// The tabs in the theme's dock style, each strip's active tab with its 3 px accent top edge.
fn show_dock(ui: &mut egui::Ui, dock: &mut DockState<Tab>, panels: &mut Panels) {
    panels.tab_rects.clear();
    DockArea::new(dock)
        .style(theme::dock_style(ui.style()))
        .show_close_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_inside(ui, panels);
    // egui_dock shows this cursor only while a tab is dragged; its edge would stay behind.
    if ui
        .ctx()
        .output(|o| o.cursor_icon == egui::CursorIcon::Grabbing)
    {
        return;
    }
    for (_, leaf) in dock.iter_leaves() {
        let active = leaf.tabs.get(leaf.active.0);
        for (_, rect) in panels.tab_rects.iter().filter(|(t, _)| Some(t) == active) {
            if rect.is_positive() {
                theme::active_tab_edge(ui.painter(), *rect);
            }
        }
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
                // Nor its proposals, which the Proposals tab counts.
                Reply::Proposals(_) | Reply::ProposalFailed(_) if retired => {}
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
                    self.shared.catalogue_loads += 1;
                    self.shared.datasheets = loaded.datasheets;
                    self.shared.valve_models = loaded.valve_models;
                    self.shared.block_models = loaded.block_models;
                    self.models.retry_failed();
                    self.shared.error = None;
                    // Connected: the waiting count shows before the Proposals tab is opened.
                    self.proposals.load(&self.shared);
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
            self.proposals = proposals::ProposalsPanel::default();
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
        status_strip(
            ui,
            &mut panels.shared,
            panels.signin.as_mut(),
            self.update.as_ref(),
        );
        show_dock(ui, &mut self.dock, &mut self.panels);
        if let Some(s) = &mut self.panels.signin {
            s.ui(ui.ctx(), &mut self.panels.shared);
        }
    }
}

/// The strip above the tabs, so database errors show whichever tab is open: the error as a
/// banner, then the latest notice and the update link at the left and who is signed in at the
/// right. Shown only when there is something to show.
fn status_strip(
    ui: &mut egui::Ui,
    shared: &mut Shared,
    signin: Option<&mut SignIn>,
    update: Option<&Update>,
) {
    let notice = shared.notice.clone();
    // Signed out, the sign-in window asks; the strip has nothing to say about it.
    let signin = signin.filter(|s| s.user().is_some());
    if shared.error.is_none() && notice.is_none() && signin.is_none() && update.is_none() {
        return;
    }
    theme::section(ui, |ui| {
        if let Some(e) = &shared.error {
            // The first sentence in bold, as banners read.
            let (first, rest) = match e.split_once(". ") {
                Some((first, rest)) => (format!("{first}."), rest),
                None => (e.clone(), ""),
            };
            theme::banner(ui, Mark::Error, &first, rest, |_| ());
        }
        if notice.is_none() && signin.is_none() && update.is_none() {
            return;
        }
        egui::Sides::new().shrink_left().wrap().show(
            ui,
            |ui| {
                if let Some(n) = notice {
                    ui.label(RichText::new(n).color(theme::LIGHT.secondary));
                }
                if let Some(u) = update {
                    let link = ui.hyperlink_to(format!("Download Hydra {}", u.version), &u.url);
                    if let Some(notes) = &u.notes {
                        link.on_hover_text(notes);
                    }
                }
            },
            |ui| {
                if let Some(s) = signin {
                    s.status(ui, shared);
                }
            },
        );
    });
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
            theme::apply(ctx, &theme::LIGHT);
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
                        shared.catalogue_loads += 1;
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
                    tab_rects: Vec::new(),
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
            tab_rects: Vec::new(),
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
            tab_rects: Vec::new(),
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
            tab_rects: Vec::new(),
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

    #[test]
    fn the_proposals_tab_counts_what_waits_for_review() {
        use hydra_db::ProposalStatus::{Approved, Pending};
        let record = proposals::tests::record;
        let ctx = egui::Context::default();
        let (mut panels, inbox, outbox) = panels_with_db(None);
        let title = |p: &mut Panels| p.title(&mut Tab::Proposals).text().to_string();
        // Not loaded yet: no count, not a guess.
        assert_eq!(title(&mut panels), "Proposals");
        outbox
            .send(Reply::Proposals(vec![
                record(Pending),
                record(Approved),
                record(Pending),
            ]))
            .unwrap();
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals 2 ");
        let egui::WidgetText::LayoutJob(job) = panels.title(&mut Tab::Proposals) else {
            panic!("a title with a badge")
        };
        assert_eq!(job.sections[1].format.background, theme::LIGHT.agent.strong);
        // A review fetches the list again, and the count follows it.
        outbox.send(Reply::ProposalRejected).unwrap();
        outbox
            .send(Reply::Proposals(vec![record(Pending)]))
            .unwrap();
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals 1 ");
        outbox
            .send(Reply::Proposals(vec![record(Approved)]))
            .unwrap();
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals");
        // Signed out, an old sign-in's count doesn't show.
        outbox
            .send(Reply::Proposals(vec![record(Pending)]))
            .unwrap();
        panels.poll_db(&ctx);
        let old = panels.shared.db.take().unwrap();
        panels.shared.retired.push(old.retire());
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals");
        // The old worker's late list changes nothing once someone signs in again.
        outbox
            .send(Reply::Proposals(vec![record(Pending), record(Pending)]))
            .unwrap();
        let (requests, new_inbox) = channel();
        let (new_outbox, replies) = channel();
        panels.shared.db = Some(Db::new(requests, replies));
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals");
        drop(inbox);
        // Connected: the count loads without opening the Proposals tab, once.
        new_outbox
            .send(Reply::Catalogue(Default::default()))
            .unwrap();
        panels.poll_db(&ctx);
        let asked = |inbox: &Receiver<Request>| {
            inbox
                .try_iter()
                .filter(|r| matches!(r, Request::ListProposals { pending_only: true }))
                .count()
        };
        assert_eq!(asked(&new_inbox), 1);
        new_outbox
            .send(Reply::Proposals(vec![record(Pending)]))
            .unwrap();
        new_outbox
            .send(Reply::Catalogue(Default::default()))
            .unwrap();
        panels.poll_db(&ctx);
        assert_eq!(title(&mut panels), "Proposals 1 ");
        assert_eq!(asked(&new_inbox), 0, "loaded once, not on every catalogue");
    }

    #[test]
    fn the_active_tab_has_the_accent_edge_and_the_panel_colour() {
        type Dock = (DockState<Tab>, Panels);
        let dock = DockState::new(vec![Tab::Schematic, Tab::Catalogue, Tab::Account]);
        let mut h = theme::harness(
            egui::vec2(900.0, 500.0),
            |ui, (dock, panels): &mut Dock| show_dock(ui, dock, panels),
            (dock, panels_with_db(None).0),
        );
        h.run();
        let check = |h: &egui_kittest::Harness<Dock>, active: Tab| {
            let rects = &h.state().1.tab_rects;
            assert_eq!(rects.len(), 3, "every tab was drawn");
            let shapes: Vec<_> = (h.output().shapes.iter())
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) => Some((r.rect, r.fill)),
                    _ => None,
                })
                .collect();
            for (tab, rect) in rects {
                let edge = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 3.0));
                let edged = shapes.contains(&(edge, theme::LIGHT.accent));
                assert_eq!(edged, *tab == active, "accent edge on {}", *tab as u8);
                let fill = if *tab == active {
                    theme::LIGHT.panel
                } else {
                    theme::LIGHT.chrome
                };
                assert!(
                    shapes.contains(&(*rect, fill)),
                    "tab {} on {fill:?}",
                    *tab as u8
                );
            }
        };
        check(&h, Tab::Schematic);
        let (dock, _) = h.state_mut();
        let found = dock.find_tab(&Tab::Account).unwrap();
        dock.set_active_tab(found).unwrap();
        h.run();
        check(&h, Tab::Account);

        // While a tab is dragged, no edge is left at its old place.
        let tab = h.state().1.tab_rects[2].1;
        h.drag_at(tab.center());
        h.run();
        h.hover_at(tab.center() + egui::vec2(200.0, 150.0));
        h.run();
        let edges = (h.output().shapes.iter())
            .filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == theme::LIGHT.accent && r.rect.height() == 3.0))
            .count();
        assert_eq!(edges, 0, "an edge while a tab is dragged");
        h.drop_at(tab.center());
        h.run();
        // Tabs stay on the main surface, where the edge is painted.
        assert!(!h.state_mut().1.allowed_in_windows(&mut Tab::View3d));
    }

    type Strip = (Shared, Option<SignIn>, Option<Update>);

    fn strip(state: Strip) -> egui_kittest::Harness<'static, Strip> {
        let mut h = theme::harness(
            egui::vec2(900.0, 200.0),
            |ui, (shared, signin, update): &mut Strip| {
                status_strip(ui, shared, signin.as_mut(), update.as_ref());
            },
            state,
        );
        h.run();
        h
    }

    #[test]
    fn the_status_strip_shows_an_error_banner_the_notice_and_the_update_link() {
        use egui_kittest::kittest::Queryable;
        let mut shared = Shared::new(Catalogue::default());
        shared.error = Some("Could not reach the database. Check the network.".into());
        shared.notice = Some("Saved vt-a@2.".into());
        let update = Update {
            version: "0.2.0".into(),
            url: "https://example.invalid/hydra".into(),
            notes: None,
        };
        let h = strip((shared, Some(SignIn::test("anna", "pw")), Some(update)));
        // The error as a banner in the missing tone, its first sentence first.
        h.get_by_label("Could not reach the database. Check the network.");
        let tone = theme::LIGHT.missing;
        let banner = h.output().shapes.iter().any(|c| {
            matches!(&c.shape, egui::Shape::Rect(r) if r.fill == tone.tint && r.stroke.color == tone.strong)
        });
        assert!(banner, "no banner in the missing tone");
        h.get_by_label("Saved vt-a@2.");
        // The update as a link, in the accent.
        h.get_by_label("Download Hydra 0.2.0");
        let accent = h.output().shapes.iter().any(|c| {
            matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "Download Hydra 0.2.0"
                && [t.fallback_color, t.galley.job.sections[0].format.color]
                    .contains(&theme::LIGHT.accent))
        });
        assert!(accent, "the update link is in the accent");
        // At the right, as in the app bar: the database, who, then Sign out.
        let x = |label: &str| h.get_by_label(label).rect().left();
        assert!(x("hydra") < x("Signed in as anna"));
        assert!(x("Signed in as anna") < x("Sign out"));
        assert!(x("Download Hydra 0.2.0") < x("hydra"));
    }

    #[test]
    fn the_status_strip_is_hidden_with_nothing_to_show() {
        let lines = |h: &egui_kittest::Harness<Strip>| {
            h.output()
                .shapes
                .iter()
                .filter(|c| matches!(c.shape, egui::Shape::LineSegment { .. }))
                .count()
        };
        let h = strip((Shared::new(Catalogue::default()), None, None));
        assert_eq!(lines(&h), 0, "no strip, so no line under it");
        // Signed out, the sign-in window asks: no empty strip above it.
        let signed_out = SignIn::new("postgres://hydra-db.office.local:5433/hydra".into()).unwrap();
        let h = strip((Shared::new(Catalogue::default()), Some(signed_out), None));
        assert_eq!(lines(&h), 0, "no empty strip while signed out");
        let h = strip((
            Shared::new(Catalogue::default()),
            Some(SignIn::test("anna", "pw")),
            None,
        ));
        assert_eq!(lines(&h), 1, "signed in, the strip shows who");
    }
}
