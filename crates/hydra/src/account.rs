//! Account tab: how the app is connected, the user's own password and, for administrators, the
//! Hydra users (Postgres login roles in `hydra_engineer`). Postgres enforces who may do what;
//! the tab only disables what it knows the server would refuse.

use std::cell::Cell;

use eframe::egui::{self, Align, Color32, Layout, RichText};
use hydra_db::{Account, MIN_PASSWORD_CHARS, User};

use crate::catalogue::Shared;
use crate::db::{Request, UserChange};
use crate::signin::SignIn;
use crate::theme::{self, LIGHT, Mark};
use crate::{DATABASE_URL_VAR, config};

#[derive(Default)]
pub struct AccountPanel {
    /// `None` until the current connection's account arrives.
    account: Option<Account>,
    /// Administrators only; `None` until the first list arrives.
    users: Option<Vec<User>>,
    /// An account or user request is waiting for its reply.
    busy: bool,
    /// A user request failed or was refused before sending.
    error: Option<String>,
    password_error: Option<String>,
    current: String,
    new: String,
    repeat: String,
    /// The new own password, until the server confirms the change.
    changing: Option<String>,
    add_name: String,
    add_password: String,
    add_admin: bool,
    /// The Set password window: whose, and the new password.
    set_password: Option<(String, String)>,
    /// Why the Set password window can't send yet.
    set_password_error: Option<String>,
    /// The Delete window: whom.
    delete: Option<String>,
}

impl AccountPanel {
    /// The current connection's account. Starts afresh, and administrators load the users.
    pub fn signed_in(&mut self, account: Account, shared: &Shared) {
        let admin = account.admin;
        *self = AccountPanel {
            account: Some(account),
            ..AccountPanel::default()
        };
        if admin {
            self.send(Request::ListUsers, shared);
        }
    }

    /// Signed out or offline: nothing of the last sign-in stays.
    pub fn forget(&mut self) {
        *self = AccountPanel::default();
    }

    /// The worker's answer to [`Request::ListUsers`], also sent after every change.
    pub fn users(&mut self, list: Vec<User>) {
        self.busy = false;
        self.error = None;
        // The user just added is in the list: clear the form.
        if list.iter().any(|u| u.name == self.add_name.trim()) {
            self.add_name.clear();
            self.add_admin = false;
        }
        self.users = Some(list);
    }

    pub fn failed(&mut self, message: String) {
        self.busy = false;
        if self.changing.take().is_some() {
            self.password_error = Some(message);
        } else {
            self.error = Some(message);
        }
    }

    /// The own password changed; returns the new one so the app can reconnect with it.
    pub fn password_changed(&mut self) -> Option<String> {
        self.busy = false;
        self.changing.take()
    }

    fn send(&mut self, request: Request, shared: &Shared) {
        if let Some(db) = &shared.db {
            db.send(request);
            self.busy = true;
            self.error = None;
            self.password_error = None;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared, signin: Option<&mut SignIn>) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| self.contents(ui, shared, signin));
        self.windows(ui.ctx(), shared);
    }

    fn contents(&mut self, ui: &mut egui::Ui, shared: &mut Shared, signin: Option<&mut SignIn>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut signed_in = None;
        if signin.is_none() && shared.db.is_none() {
            theme::banner(
                ui,
                Mark::Warning,
                "Offline: no database.",
                &format!(
                    "Set database_url in {} or {DATABASE_URL_VAR} to use the shared database.",
                    config::FILE
                ),
                |_| (),
            );
        } else {
            signed_in = theme::section(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                theme::column_title(ui, "ACCOUNT");
                self.account_ui(ui, shared, signin)
            });
        }
        if shared.db.is_some() {
            theme::section(ui, |ui| self.password_ui(ui, shared, signed_in));
            if self.account.as_ref().is_some_and(|a| a.admin) {
                theme::section(ui, |ui| self.users_ui(ui, shared));
            }
        }
        theme::section(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            theme::column_title(ui, "ABOUT");
            ui.monospace(format!("Hydra {}", env!("CARGO_PKG_VERSION")));
            // The licence travels with the fonts compiled into the binary (OFL section 2).
            egui::CollapsingHeader::new("Font licence: IBM Plex")
                .id_salt("font-licence")
                .show(ui, |ui| {
                    ui.label(include_str!("../fonts/OFL.txt"));
                });
        });
    }

    /// Who is signed in, and through which database. Returns the sign-in while signed in.
    fn account_ui<'a>(
        &self,
        ui: &mut egui::Ui,
        shared: &mut Shared,
        signin: Option<&'a mut SignIn>,
    ) -> Option<&'a SignIn> {
        let secondary = |text: String| RichText::new(text).color(LIGHT.secondary);
        match signin {
            Some(s) => match s.user().map(str::to_owned) {
                Some(user) => {
                    ui.label(
                        RichText::new(format!("Signed in as {user}")).text_style(theme::heading()),
                    );
                    ui.label(secondary(s.server()).monospace());
                    ui.label(secondary(
                        if s.remembered() {
                            "Your password is remembered on this computer."
                        } else {
                            "Your password is not remembered on this computer."
                        }
                        .into(),
                    ));
                    ui.add_space(2.0);
                    if ui
                        .add(egui::Button::new("Sign out").min_size(egui::vec2(0.0, 32.0)))
                        .on_hover_text("Also forgets the remembered password.")
                        .clicked()
                    {
                        s.sign_out(shared);
                    }
                    Some(s)
                }
                None => {
                    ui.label("Signed out.");
                    None
                }
            },
            None => {
                ui.label(match &self.account {
                    Some(a) => format!("Connected as {} through {DATABASE_URL_VAR}", a.user),
                    None => format!("Connecting through {DATABASE_URL_VAR}…"),
                });
                None
            }
        }
    }

    /// The own password: labelled fields, checked before Change password sends them.
    fn password_ui(&mut self, ui: &mut egui::Ui, shared: &Shared, signed_in: Option<&SignIn>) {
        ui.spacing_mut().item_spacing.y = 8.0;
        theme::column_title(ui, "YOUR PASSWORD");
        let busy = self.busy;
        if signed_in.is_some() {
            password_field(ui, "Current password", &mut self.current);
        }
        password_field(ui, "New password", &mut self.new);
        password_field(ui, "Repeat new password", &mut self.repeat);
        let ready =
            !busy && !self.new.is_empty() && (signed_in.is_none() || !self.current.is_empty());
        let change = ui
            .add_enabled(ready, theme::primary("Change password"))
            .on_disabled_hover_text(if busy {
                WAITING
            } else {
                "Fill in the passwords first."
            });
        if change.clicked() {
            let problem = if signed_in.is_some_and(|s| !s.is_password(&self.current)) {
                Some("The current password is wrong.".to_string())
            } else if self.new != self.repeat {
                Some("The new passwords don't match.".to_string())
            } else {
                too_short(&self.new)
            };
            match problem {
                Some(p) => self.password_error = Some(p),
                None => {
                    self.send(Request::ChangePassword(self.new.clone()), shared);
                    self.changing = Some(self.new.clone());
                }
            }
            self.current.clear();
            self.new.clear();
            self.repeat.clear();
        }
        if let Some(e) = &self.password_error {
            theme::marked(ui, Mark::Error, e.as_str());
        }
        let admin = self.account.as_ref().is_some_and(|a| a.admin);
        if let Some(e) = self.error.as_ref().filter(|_| !admin) {
            theme::marked(ui, Mark::Error, e.as_str());
        }
    }

    /// The users as a table with each one's actions, then the form for a new user.
    fn users_ui(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        ui.spacing_mut().item_spacing.y = 8.0;
        let busy = self.busy;
        ui.horizontal(|ui| {
            theme::column_title(ui, "USERS");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Refresh"))
                    .on_disabled_hover_text(WAITING)
                    .clicked()
                {
                    self.send(Request::ListUsers, shared);
                }
                if self.busy {
                    ui.spinner();
                }
            });
        });
        if let Some(e) = &self.error {
            theme::marked(ui, Mark::Error, e.as_str());
        }
        let me = self.account.as_ref().map_or("", |a| a.user.as_str());
        let mut request = None;
        let (mut set_password, mut delete) = (None, None);
        if let Some(users) = &self.users {
            // Name takes the slack: the table's edges, the rows' 12 px margins and the gaps come
            // off. Below its narrowest the table scrolls sideways, so Actions stays reachable.
            let [role, login, sessions, actions] = [100.0, 92.0, 64.0, 72.0];
            let fixed = 2.0 + 24.0 + 4.0 * 12.0 + role + login + sessions + actions;
            let width = ui.available_width().max(fixed + NAME_MIN);
            let name_width = width - fixed;
            // Each cell's line is as tall as the Actions button, so the row reads level.
            let level = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                ui.horizontal(|ui| add(ui));
            };
            egui::ScrollArea::horizontal()
                .id_salt("users")
                .show(ui, |ui| {
                    theme::table(ui, width, |ui| {
                        theme::table_row(ui, LIGHT.panel, |ui| {
                            for (title, w) in [
                                ("NAME", name_width),
                                ("ROLE", role),
                                ("SIGNING IN", login),
                                ("SESSIONS", sessions),
                            ] {
                                theme::cell(ui, w, |ui| {
                                    theme::column_title(ui, title);
                                });
                            }
                        });
                        for u in users {
                            let own = u.name == me;
                            theme::table_row(ui, Color32::TRANSPARENT, |ui| {
                                theme::cell(ui, name_width, |ui| {
                                    level(ui, &mut |ui| {
                                        // Cut short when long; the whole name shows in its hover.
                                        let name = if own {
                                            format!("{} (you)", u.name)
                                        } else {
                                            u.name.clone()
                                        };
                                        ui.add(egui::Label::new(name).truncate());
                                    });
                                });
                                theme::cell(ui, role, |ui| {
                                    level(ui, &mut |ui| {
                                        ui.label(if u.admin {
                                            "Administrator"
                                        } else {
                                            "Engineer"
                                        });
                                    });
                                });
                                theme::cell(ui, login, |ui| {
                                    level(ui, &mut |ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        if u.can_login {
                                            ui.label("Allowed");
                                        } else {
                                            theme::marked(ui, Mark::Rejected, "Disabled");
                                        }
                                    });
                                });
                                theme::cell(ui, sessions, |ui| {
                                    level(ui, &mut |ui| {
                                        let count = ui.monospace(u.sessions.len().to_string());
                                        if !u.sessions.is_empty() {
                                            let lines: Vec<String> = u
                                                .sessions
                                                .iter()
                                                .map(|s| match &s.client {
                                                    Some(c) => format!(
                                                        "{} from {c} since {}",
                                                        s.application, s.since
                                                    ),
                                                    None => format!(
                                                        "{} since {}",
                                                        s.application, s.since
                                                    ),
                                                })
                                                .collect();
                                            count.on_hover_text(lines.join("\n"));
                                        }
                                    });
                                });
                                // A button, disabled with the reason when the rules forbid it on your own row.
                                let action = |ui: &mut egui::Ui, text: &str, forbidden: &str| {
                                    let b = ui.add_enabled(!busy && !own, egui::Button::new(text));
                                    let b = if own {
                                        b.on_disabled_hover_text(forbidden)
                                    } else {
                                        b
                                    };
                                    b.clicked()
                                };
                                let name = || u.name.clone();
                                // A menu: five buttons per row are wider than the tab.
                                ui.menu_button("Actions", |ui| {
                                    if action(
                                        ui,
                                        "Set password…",
                                        "Change your own password above.",
                                    ) {
                                        set_password = Some(name());
                                    }
                                    let admin = if u.admin {
                                        "Remove admin"
                                    } else {
                                        "Make admin"
                                    };
                                    if action(
                                        ui,
                                        admin,
                                        "You can't remove your own administrator rights.",
                                    ) {
                                        request = Some(UserChange::SetAdmin(name(), !u.admin));
                                    }
                                    let login = if u.can_login { "Disable" } else { "Enable" };
                                    if action(ui, login, "You can't disable yourself.") {
                                        request =
                                            Some(UserChange::SetCanLogin(name(), !u.can_login));
                                    }
                                    if action(
                                        ui,
                                        "End sessions",
                                        "You can't end your own sessions.",
                                    ) {
                                        request = Some(UserChange::EndSessions(name()));
                                    }
                                    if action(ui, "Delete…", "You can't delete yourself.") {
                                        delete = Some(name());
                                    }
                                });
                            });
                        }
                    });
                });
        }

        ui.add_space(8.0);
        ui.label(RichText::new("New user").text_style(theme::heading()));
        let name = egui::TextEdit::singleline(&mut self.add_name).desired_width(280.0);
        theme::field(ui, "Name", name);
        password_field(ui, "Password", &mut self.add_password);
        ui.checkbox(&mut self.add_admin, "Administrator");
        let ready = !busy && !self.add_name.trim().is_empty() && !self.add_password.is_empty();
        let add = ui
            .add_enabled(ready, theme::primary("Add user"))
            .on_disabled_hover_text(if busy {
                WAITING
            } else {
                "Enter the name and password first."
            });
        if add.clicked() {
            let password = std::mem::take(&mut self.add_password);
            match too_short(&password) {
                Some(p) => self.error = Some(p),
                None => {
                    request = Some(UserChange::Create {
                        name: self.add_name.trim().to_string(),
                        password,
                        admin: self.add_admin,
                    })
                }
            }
        }

        if let Some(change) = request {
            self.send(Request::ChangeUser(change), shared);
        }
        if let Some(name) = set_password {
            self.set_password = Some((name, String::new()));
            self.set_password_error = None;
        }
        if delete.is_some() {
            self.delete = delete;
        }
    }

    /// The Set password and Delete windows.
    fn windows(&mut self, ctx: &egui::Context, shared: &Shared) {
        if let Some((name, password)) = &mut self.set_password {
            let ready = Cell::new(false);
            let error = &self.set_password_error;
            let modal = theme::dialog(
                ctx,
                "set-password",
                &format!("Set the password of {name}"),
                |ui| {
                    password_field(ui, "New password", password);
                    ui.label(
                        RichText::new("Their open sessions stay signed in.").color(LIGHT.secondary),
                    );
                    if let Some(e) = error {
                        theme::marked(ui, Mark::Error, e.as_str());
                    }
                    ready.set(!password.is_empty());
                },
                |ui| {
                    let set = ui
                        .add_enabled(ready.get(), theme::primary("Set password"))
                        .on_disabled_hover_text("Enter the new password first.")
                        .clicked();
                    (set, cancel_button(ui))
                },
            );
            let (set, cancel) = modal.inner;
            if set {
                let (name, password) = self.set_password.take().expect("window is open");
                match too_short(&password) {
                    Some(p) => {
                        self.set_password_error = Some(p);
                        self.set_password = Some((name, password));
                    }
                    None => self.send(
                        Request::ChangeUser(UserChange::SetPassword(name, password)),
                        shared,
                    ),
                }
            } else if cancel || modal.should_close() {
                self.set_password = None;
            }
        }
        if let Some(name) = &self.delete {
            let modal = theme::dialog(
                ctx,
                "delete-user",
                &format!("Delete {name}?"),
                |ui| {
                    ui.label(
                        RichText::new(
                            "They are signed out and can no longer sign in. Their name stays on \
                             the revisions they saved.",
                        )
                        .color(LIGHT.secondary),
                    );
                },
                |ui| {
                    let confirmed = ui.add(theme::danger(format!("Delete {name}"))).clicked();
                    (confirmed, cancel_button(ui))
                },
            );
            let (confirmed, cancel) = modal.inner;
            if confirmed {
                let name = self.delete.take().expect("window is open");
                self.send(Request::ChangeUser(UserChange::Delete(name)), shared);
            } else if cancel || modal.should_close() {
                self.delete = None;
            }
        }
    }
}

/// A labelled password field, 280 px wide.
fn password_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    let edit = egui::TextEdit::singleline(value)
        .password(true)
        .desired_width(280.0);
    theme::field(ui, label, edit);
}

/// A dialog's Cancel button, 32 px high like the others. Returns whether it was clicked.
fn cancel_button(ui: &mut egui::Ui) -> bool {
    ui.add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 32.0)))
        .clicked()
}

const WAITING: &str = "Waiting for the database.";
/// The users table's name column at its narrowest.
const NAME_MIN: f32 = 72.0;

fn too_short(password: &str) -> Option<String> {
    (password.chars().count() < MIN_PASSWORD_CHARS)
        .then(|| format!("Passwords need at least {MIN_PASSWORD_CHARS} characters."))
}

#[cfg(test)]
impl AccountPanel {
    pub fn set_changing(&mut self, password: &str) {
        self.changing = Some(password.into());
    }

    pub fn shown(&self) -> (Option<&Account>, Option<&Vec<User>>) {
        (self.account.as_ref(), self.users.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use eframe::egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use hydra_db::Session;

    use super::*;
    use crate::db::Db;

    type State = (AccountPanel, Shared, Option<SignIn>);

    fn user(name: &str, admin: bool, sessions: usize) -> User {
        User {
            name: name.into(),
            admin,
            can_login: true,
            sessions: (0..sessions)
                .map(|i| Session {
                    pid: 100 + i as i32,
                    application: "Hydra".into(),
                    client: Some("10.0.0.7".into()),
                    since: "2026-10-30 08:15".into(),
                })
                .collect(),
        }
    }

    /// The tab signed in through hydra.ini as `name`, with the requests it sends.
    fn harness(name: &str, admin: bool) -> (Harness<'static, State>, Receiver<Request>) {
        harness_wide(name, admin, 900.0)
    }

    fn harness_wide(
        name: &str,
        admin: bool,
        width: f32,
    ) -> (Harness<'static, State>, Receiver<Request>) {
        let (requests, inbox) = channel();
        let mut shared = Shared::new(hydra_core::Catalogue::default());
        shared.db = Some(Db::new(requests, channel().1));
        let mut panel = AccountPanel::default();
        let account = Account {
            user: name.into(),
            admin,
        };
        panel.signed_in(account, &shared);
        let signin = SignIn::test(name, "old-password-1");
        let mut h = theme::harness(
            egui::vec2(width, 1000.0),
            |ui, (p, shared, signin): &mut State| p.ui(ui, shared, signin.as_mut()),
            (panel, shared, Some(signin)),
        );
        h.run_steps(4);
        (h, inbox)
    }

    /// Open the Actions menu of the `row`th user.
    fn open_actions(h: &mut Harness<State>, row: usize) {
        h.query_all_by_label("Actions").nth(row).unwrap().click();
        h.run_steps(4);
    }

    // run_steps, not run: the tab shows a spinner while a request is out.
    fn disabled(h: &Harness<State>, label: &str, index: usize) -> bool {
        h.query_all_by_label(label)
            .nth(index)
            .unwrap_or_else(|| panic!("no {label} #{index}"))
            .accesskit_node()
            .is_disabled()
    }

    fn shapes(h: &Harness<State>) -> Vec<egui::Shape> {
        h.output().shapes.iter().map(|c| c.shape.clone()).collect()
    }

    fn rects<S>(h: &Harness<S>) -> Vec<egui::epaint::RectShape> {
        h.output()
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_engineer_sees_the_connection_but_no_users() {
        let (h, inbox) = harness("anna", false);
        h.get_by_label("Signed in as anna");
        h.get_by_label("Database hydra on hydra-db.office.local:5433");
        h.get_by_label("Your password is not remembered on this computer.");
        h.get_by_label("Sign out");
        h.get_by_label(&format!("Hydra {}", env!("CARGO_PKG_VERSION")));
        // Passwords are masked.
        h.get_by_role_and_label(Role::PasswordInput, "Current password");
        h.get_by_role_and_label(Role::PasswordInput, "New password");
        assert!(h.query_by_label("USERS").is_none());
        assert!(h.query_by_label("Add user").is_none());
        assert!(inbox.try_recv().is_err(), "an engineer loads no users");
    }

    #[test]
    fn offline_shows_how_to_connect() {
        let mut h = theme::harness(
            egui::vec2(900.0, 400.0),
            |ui, (p, shared): &mut (AccountPanel, Shared)| p.ui(ui, shared, None),
            (
                AccountPanel::default(),
                Shared::new(hydra_core::Catalogue::default()),
            ),
        );
        h.run_steps(4);
        // One banner in the unlisted tone: what is wrong, then how to fix it.
        h.get_by_label(
            "Offline: no database. Set database_url in hydra.ini or HYDRA_DATABASE_URL to use \
             the shared database.",
        );
        let tone = LIGHT.unlisted;
        assert!(
            rects(&h)
                .iter()
                .any(|r| r.fill == tone.tint && r.stroke.color == tone.strong),
            "no banner in the unlisted tone"
        );
        assert!(h.query_by_label("Change password").is_none());
        assert!(h.query_by_label("ACCOUNT").is_none());
        h.get_by_label(&format!("Hydra {}", env!("CARGO_PKG_VERSION")));
        // The bundled fonts' licence can be read.
        assert!(h.query_by_label_contains("SIL OPEN FONT LICENSE").is_none());
        h.get_by_label("Font licence: IBM Plex").click();
        h.run_steps(4);
        h.get_by_label_contains("SIL OPEN FONT LICENSE Version 1.1");
    }

    #[test]
    fn an_admin_manages_others_but_not_their_own_row() {
        let (mut h, inbox) = harness("boss", true);
        assert!(matches!(inbox.try_recv(), Ok(Request::ListUsers)));
        let list = vec![user("anna", false, 2), user("boss", true, 1)];
        h.state_mut().0.users(list.clone());
        h.run_steps(4);
        h.get_by_label("anna");
        h.get_by_label("boss (you)");
        h.get_by_label("2");
        // Rows are anna, then boss: every action on boss's own row that the rules forbid is off.
        open_actions(&mut h, 1);
        for label in [
            "Set password…",
            "Remove admin",
            "Disable",
            "End sessions",
            "Delete…",
        ] {
            assert!(disabled(&h, label, 0), "{label} for boss");
        }
        h.key_press(egui::Key::Escape);
        h.run_steps(4);
        open_actions(&mut h, 0);
        for label in [
            "Set password…",
            "Make admin",
            "Disable",
            "End sessions",
            "Delete…",
        ] {
            assert!(!disabled(&h, label, 0), "{label} for anna");
        }
        h.get_by_label("Disable").click();
        h.run_steps(4);
        assert!(matches!(
            inbox.try_recv(),
            Ok(Request::ChangeUser(UserChange::SetCanLogin(n, false))) if n == "anna"
        ));
        assert!(disabled(&h, "Refresh", 0), "busy until the list reloads");

        h.state_mut().0.users(list);
        h.run_steps(4);
        assert!(!disabled(&h, "Refresh", 0));
        open_actions(&mut h, 0);
        h.get_by_label("Delete…").click();
        h.run_steps(4);
        assert!(inbox.try_recv().is_err(), "Delete asks first");
        h.get_by_label("Delete anna?");
        h.get_by_label("Delete anna").click();
        h.run_steps(4);
        assert!(matches!(
            inbox.try_recv(),
            Ok(Request::ChangeUser(UserChange::Delete(n))) if n == "anna"
        ));
        assert!(h.query_by_label("Delete anna?").is_none());
    }

    #[test]
    fn adding_a_user_checks_the_password_and_shows_refusals() {
        let (mut h, inbox) = harness("boss", true);
        let _ = inbox.try_recv();
        h.state_mut().0.users(vec![user("boss", true, 1)]);
        h.run_steps(4);
        h.get_by_role_and_label(Role::PasswordInput, "Password");
        h.state_mut().0.add_name = " Carl ".into();
        h.state_mut().0.add_password = "short".into();
        h.run_steps(4);
        h.get_by_label("Add user").click();
        h.run_steps(4);
        h.get_by_label("Passwords need at least 10 characters.");
        assert!(inbox.try_recv().is_err());
        assert!(h.state().0.add_password.is_empty());

        h.state_mut().0.add_password = "long enough 1".into();
        h.state_mut().0.add_admin = true;
        h.run_steps(4);
        h.get_by_label("Add user").click();
        h.run_steps(4);
        let Ok(Request::ChangeUser(UserChange::Create {
            name,
            password,
            admin,
        })) = inbox.try_recv()
        else {
            panic!("expected Create")
        };
        assert_eq!(
            (&name[..], &password[..], admin),
            ("Carl", "long enough 1", true)
        );
        // The server's rules come back in plain words.
        h.state_mut()
            .0
            .failed("user names are lowercase letters, digits and _".into());
        h.run_steps(4);
        h.get_by_label("user names are lowercase letters, digits and _");
        assert!(!disabled(&h, "Refresh", 0));

        // Once the new user is listed, the form clears.
        h.state_mut().0.add_name = "carl".into();
        h.state_mut()
            .0
            .users(vec![user("boss", true, 1), user("carl", true, 0)]);
        assert!(h.state().0.add_name.is_empty());
    }

    #[test]
    fn the_own_password_is_checked_before_it_is_sent() {
        let (mut h, inbox) = harness("anna", false);
        let attempt = |h: &mut Harness<State>, current: &str, new: &str, repeat: &str| {
            let p = &mut h.state_mut().0;
            (p.current, p.new, p.repeat) = (current.into(), new.into(), repeat.into());
            h.run_steps(4);
            h.get_by_label("Change password").click();
            h.run_steps(4);
            let p = &h.state().0;
            assert!(p.current.is_empty() && p.new.is_empty() && p.repeat.is_empty());
        };
        attempt(&mut h, "not-it", "new-password-1", "new-password-1");
        h.get_by_label("The current password is wrong.");
        attempt(&mut h, "old-password-1", "new-password-1", "new-password-2");
        h.get_by_label("The new passwords don't match.");
        attempt(&mut h, "old-password-1", "short", "short");
        h.get_by_label("Passwords need at least 10 characters.");
        assert!(inbox.try_recv().is_err(), "nothing was sent");

        attempt(&mut h, "old-password-1", "new-password-1", "new-password-1");
        assert!(
            matches!(inbox.try_recv(), Ok(Request::ChangePassword(p)) if p == "new-password-1")
        );
        assert!(h.query_by_label("The current password is wrong.").is_none());
        assert_eq!(
            h.state_mut().0.password_changed().as_deref(),
            Some("new-password-1")
        );
    }

    #[test]
    fn a_refused_password_change_shows_under_the_form() {
        let (mut h, inbox) = harness("anna", false);
        h.state_mut().0.current = "old-password-1".into();
        h.state_mut().0.new = "new-password-1".into();
        h.state_mut().0.repeat = "new-password-1".into();
        h.run_steps(4);
        h.get_by_label("Change password").click();
        h.run_steps(4);
        assert!(inbox.try_recv().is_ok());
        h.state_mut().0.failed("permission denied".into());
        h.run_steps(4);
        h.get_by_label("permission denied");
        assert_eq!(h.state_mut().0.password_changed(), None);
    }

    #[test]
    fn a_short_password_keeps_the_set_password_window_open() {
        let (mut h, inbox) = harness("boss", true);
        let _ = inbox.try_recv();
        h.state_mut()
            .0
            .users(vec![user("anna", false, 0), user("boss", true, 0)]);
        h.run_steps(4);
        open_actions(&mut h, 0);
        h.get_by_label("Set password…").click();
        h.run_steps(4);
        h.get_by_label("Set the password of anna");
        h.state_mut().0.set_password.as_mut().unwrap().1 = "short".into();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Set password")
            .click();
        h.run_steps(4);
        h.get_by_label("Passwords need at least 10 characters.");
        h.get_by_label("Set the password of anna");
        assert_eq!(h.state().0.set_password.as_ref().unwrap().1, "short");
        assert!(inbox.try_recv().is_err());
    }

    #[test]
    fn in_a_narrow_tab_a_long_name_is_cut_short_and_actions_stay_reachable() {
        let (mut h, inbox) = harness_wide("boss", true, 500.0);
        let _ = inbox.try_recv();
        let long = "a_very_long_user_name_here";
        h.state_mut()
            .0
            .users(vec![user(long, false, 0), user("boss", true, 0)]);
        h.run_steps(4);
        let actions = h.query_all_by_label("Actions").next().unwrap().rect();
        assert!(
            actions.right() <= 500.0,
            "Actions is off-screen: {actions:?}"
        );
        // The name is cut short, not pushing the row past its headers.
        let name = h.get_by_label(long).rect();
        let role = h.get_by_label("ROLE").rect();
        assert!(
            name.right() < role.left(),
            "{name:?} runs into ROLE {role:?}"
        );
        for (title, value) in [("ROLE", "Engineer"), ("SESSIONS", "0")] {
            let (t, v) = (
                h.get_by_label(title).rect(),
                h.query_all_by_label(value).next().unwrap().rect(),
            );
            assert!(
                (t.left() - v.left()).abs() <= 1.0,
                "{title} {t:?} over {value} {v:?}"
            );
        }
        open_actions(&mut h, 0);
        h.get_by_label("End sessions").click();
        h.run_steps(4);
        assert!(matches!(
            inbox.try_recv(),
            Ok(Request::ChangeUser(UserChange::EndSessions(n))) if n == long
        ));
    }

    #[test]
    fn escape_closes_the_delete_dialog_without_deleting() {
        let (mut h, inbox) = harness("boss", true);
        let _ = inbox.try_recv();
        h.state_mut()
            .0
            .users(vec![user("anna", false, 0), user("boss", true, 0)]);
        h.run_steps(4);
        open_actions(&mut h, 0);
        h.get_by_label("Delete…").click();
        h.run_steps(4);
        h.get_by_label("Delete anna?");
        h.key_press(egui::Key::Escape);
        h.run_steps(4);
        assert!(h.query_by_label("Delete anna?").is_none());
        assert!(h.state().0.delete.is_none());
        assert!(inbox.try_recv().is_err(), "nothing was sent");
    }

    #[test]
    fn password_problems_are_marked_lines_in_the_missing_tone() {
        let (mut h, _inbox) = harness("anna", false);
        let p = &mut h.state_mut().0;
        (p.current, p.new, p.repeat) = (
            "old-password-1".into(),
            "new-password-1".into(),
            "new-password-2".into(),
        );
        h.run_steps(4);
        h.get_by_label("Change password").click();
        h.run_steps(4);
        let painted = shapes(&h);
        let line = painted.iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text() == "The new passwords don't match." => Some(t),
            _ => None,
        });
        let line = line.expect("the problem in words");
        let missing = LIGHT.missing.text;
        assert_eq!(line.galley.job.sections[0].format.color, missing);
        // The ■ before it, as tall as the text and in the same tone.
        let mark = rects(&h)
            .into_iter()
            .find(|r| r.fill == missing)
            .expect("the ■ mark");
        assert!(mark.rect.right() < line.visual_bounding_rect().left());
    }

    #[test]
    fn the_users_table_says_role_and_signing_in_and_keeps_your_own_row_safe() {
        let (mut h, inbox) = harness("boss", true);
        let _ = inbox.try_recv();
        let mut carl = user("carl", false, 0);
        carl.can_login = false;
        h.state_mut()
            .0
            .users(vec![user("anna", false, 2), user("boss", true, 1), carl]);
        h.run_steps(4);
        for title in ["NAME", "ROLE", "SIGNING IN", "SESSIONS"] {
            h.get_by_label(title);
        }
        assert_eq!(h.query_all_by_label("Engineer").count(), 2);
        h.get_by_role_and_label(Role::Label, "Administrator");
        assert_eq!(h.query_all_by_label("Allowed").count(), 2);
        // Only what is wrong is marked: carl can't sign in.
        h.get_by_label("Disabled");
        let rejected = LIGHT.secondary;
        assert!(
            shapes(&h).iter().any(|s| matches!(
                s,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == rejected
            )),
            "the ✕ mark beside Disabled"
        );
        assert_eq!(h.query_all_by_label("Actions").count(), 3);
        // Your own row's forbidden actions are off and say why.
        open_actions(&mut h, 1);
        assert!(disabled(&h, "Delete…", 0));
        h.get_by_label("Delete…").hover();
        h.run_steps(30);
        h.get_by_label("You can't delete yourself.");
    }
}
