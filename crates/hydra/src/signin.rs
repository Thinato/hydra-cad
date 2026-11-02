//! Signing in to the shared database with the user's own Postgres role, so revisions record
//! who made them. A remembered password lives in the OS keychain, one entry per database.

use std::cell::Cell;

use eframe::egui::{self, RichText};
use hydra_db::PgConnectOptions;

use crate::catalogue::Shared;
use crate::config::FILE;
use crate::db::{Db, WRONG_LOGIN};
use crate::theme::{self, LIGHT, Mark};

pub struct Credentials {
    user: String,
    password: String,
}

pub struct SignIn {
    /// `database_url` from hydra.ini, without user or password. Also names the keychain entry.
    url: String,
    options: PgConnectOptions,
    user: String,
    password: String,
    remember: bool,
    /// The sign-in window is showing.
    open: bool,
    wrong_login: bool,
    /// Signed in, or signing in, with these credentials.
    signed_in: Option<Credentials>,
    /// Put the credentials in the keychain once the database answers.
    to_remember: bool,
    /// The keychain holds the signed-in credentials.
    remembered: bool,
    /// The database could not be reached; offer to try again.
    connect_failed: bool,
}

impl SignIn {
    /// Signed out, for hydra.ini's `database_url`.
    pub fn new(url: String) -> Result<Self, String> {
        let options = url
            .parse()
            .map_err(|e| format!("database_url in {FILE} is not a Postgres URL: {e}"))?;
        Ok(SignIn {
            url,
            options,
            user: String::new(),
            password: String::new(),
            remember: true,
            open: true,
            wrong_login: false,
            signed_in: None,
            to_remember: false,
            remembered: false,
            connect_failed: false,
        })
    }

    /// Connect with the credentials remembered for this database, if any.
    pub fn start(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        if let Some(c) = load(&self.url) {
            self.connect(c, false, shared, ctx);
            self.remembered = true;
        }
    }

    fn connect(
        &mut self,
        c: Credentials,
        remember: bool,
        shared: &mut Shared,
        ctx: &egui::Context,
    ) {
        let options = self.options.clone().username(&c.user).password(&c.password);
        retire(shared);
        shared.db = Some(Db::connect(options, ctx.clone()));
        shared.error = None;
        self.open = false;
        self.wrong_login = false;
        self.connect_failed = false;
        self.user.clone_from(&c.user);
        self.signed_in = Some(c);
        self.to_remember = remember;
    }

    /// The database answered: remember the credentials if the user asked to.
    pub fn connected(&mut self) {
        if std::mem::take(&mut self.to_remember)
            && let Some(c) = &self.signed_in
        {
            self.remembered = store(&self.url, c);
        }
    }

    /// The database could not be reached with the current credentials.
    pub fn connect_failed(&mut self) {
        self.connect_failed = true;
    }

    /// Postgres refused the user name or password: forget them and ask again.
    pub fn refused(&mut self, shared: &mut Shared) {
        self.sign_out(shared);
        self.wrong_login = true;
    }

    pub fn sign_out(&mut self, shared: &mut Shared) {
        forget(&self.url);
        retire(shared);
        self.signed_in = None;
        self.to_remember = false;
        self.remembered = false;
        self.password.clear();
        self.open = true;
    }

    /// The signed-in user's own password changed: keep the keychain in step and reconnect, since
    /// new connections with the old password would fail. Returns whether the keychain lost it.
    pub fn password_changed(
        &mut self,
        password: String,
        shared: &mut Shared,
        ctx: &egui::Context,
    ) -> bool {
        let Some(old) = self.signed_in.take() else {
            return false;
        };
        let c = Credentials {
            user: old.user,
            password,
        };
        let lost = self.remembered && !replace_stored(&self.url, &c, store, forget);
        if lost {
            self.remembered = false;
        }
        self.connect(c, false, shared, ctx);
        lost
    }

    /// Signed in as `user` with `password`, without a worker or the keychain.
    #[cfg(test)]
    pub fn test(user: &str, password: &str) -> SignIn {
        let mut s = SignIn::new("postgres://hydra-db.office.local:5433/hydra".into()).unwrap();
        s.open = false;
        s.signed_in = Some(Credentials {
            user: user.into(),
            password: password.into(),
        });
        s
    }

    /// Who is signed in, or signing in.
    pub fn user(&self) -> Option<&str> {
        self.signed_in.as_ref().map(|c| c.user.as_str())
    }

    pub fn remembered(&self) -> bool {
        self.remembered
    }

    /// Whether `password` is the one the user signed in with.
    pub fn is_password(&self, password: &str) -> bool {
        self.signed_in
            .as_ref()
            .is_some_and(|c| c.password == password)
    }

    /// "Database hydra on host:port".
    pub fn server(&self) -> String {
        let o = &self.options;
        format!(
            "Database {} on {}:{}",
            o.get_database().unwrap_or("hydra"),
            o.get_host(),
            o.get_port()
        )
    }

    /// The database's name, "Signed in as …" and a Sign out button, for the status line, plus
    /// Retry after a connection failure. Laid out right to left, as at the right of the strip:
    /// the rightmost first.
    pub fn status(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let Some(c) = &self.signed_in else {
            return;
        };
        let who = format!("Signed in as {}", c.user);
        let sign_out = ui.button("Sign out").clicked();
        let retry = self.connect_failed && ui.button("Retry").clicked();
        ui.label(RichText::new(who).color(LIGHT.secondary));
        let database = self.options.get_database().unwrap_or("hydra");
        ui.label(RichText::new(database).monospace().color(LIGHT.secondary))
            .on_hover_text(self.server());
        if sign_out {
            self.sign_out(shared);
        } else if retry && let Some(c) = self.signed_in.take() {
            self.connect(c, self.to_remember, shared, ui.ctx());
        }
    }

    /// The sign-in window, while signed out.
    pub fn ui(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        if let Some((c, remember)) = self.window(ctx) {
            self.connect(c, remember, shared, ctx);
        }
    }

    /// Returns the credentials and the Remember choice when the user signs in.
    fn window(&mut self, ctx: &egui::Context) -> Option<(Credentials, bool)> {
        if !self.open {
            return None;
        }
        let mut submit = false;
        let ready = Cell::new(false);
        let server = self.server();
        let clicked = theme::dialog(
            ctx,
            "sign-in",
            "Sign in to Hydra",
            |ui| {
                ui.label(
                    RichText::new(
                        "Sign in with your own Hydra user, so the revisions you save carry \
                         your name.",
                    )
                    .color(LIGHT.secondary),
                );
                ui.label(RichText::new(server).monospace().color(LIGHT.secondary));
                ui.add_space(4.0);
                let user = egui::TextEdit::singleline(&mut self.user);
                theme::field(ui, "User name", user.desired_width(f32::INFINITY));
                let password = egui::TextEdit::singleline(&mut self.password).password(true);
                let password = theme::field(ui, "Password", password.desired_width(f32::INFINITY));
                submit = password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.checkbox(&mut self.remember, "Remember me on this computer");
                if self.wrong_login {
                    theme::marked(ui, Mark::Error, WRONG_LOGIN);
                }
                ready.set(!self.user.trim().is_empty());
            },
            |ui| {
                ui.add_enabled(ready.get(), theme::primary("Sign in"))
                    .on_disabled_hover_text("Enter your user name first.")
                    .clicked()
            },
        )
        .inner;
        submit = ready.get() && (submit || clicked);
        submit.then(|| {
            let c = Credentials {
                user: self.user.trim().to_string(),
                password: self.password.clone(),
            };
            (c, self.remember)
        })
    }
}

/// Store the new credentials; if the keychain refuses, drop the old ones so the next start asks
/// rather than failing with a stale password. Returns whether they are stored.
fn replace_stored(
    url: &str,
    c: &Credentials,
    store: impl FnOnce(&str, &Credentials) -> bool,
    forget: impl FnOnce(&str),
) -> bool {
    let stored = store(url, c);
    if !stored {
        forget(url);
    }
    stored
}

/// Stop using the current worker; its replies still arrive (see `Panels::poll_db`).
fn retire(shared: &mut Shared) {
    if let Some(db) = shared.db.take() {
        shared.retired.push(db.retire());
    }
}

// The keychain, behind three functions so no test ever touches it. Service "Hydra", account the
// database_url, secret {"user": …, "password": …}. Failures are logged; the user just signs in.

const SERVICE: &str = "Hydra";

fn load(url: &str) -> Option<Credentials> {
    let secret = match keyring::Entry::new(SERVICE, url).and_then(|e| e.get_password()) {
        Ok(s) => s,
        Err(keyring::Error::NoEntry) => return None,
        Err(e) => {
            tracing::warn!("keychain: {e}");
            return None;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&secret).ok()?;
    Some(Credentials {
        user: v["user"].as_str()?.to_string(),
        password: v["password"].as_str()?.to_string(),
    })
}

/// Returns whether the keychain took them.
fn store(url: &str, c: &Credentials) -> bool {
    let secret = serde_json::json!({"user": c.user, "password": c.password}).to_string();
    match keyring::Entry::new(SERVICE, url).and_then(|e| e.set_password(&secret)) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("keychain: {e}");
            false
        }
    }
}

fn forget(url: &str) {
    match keyring::Entry::new(SERVICE, url).and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => tracing::warn!("keychain: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::accesskit::Role;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    use super::*;

    type State = (SignIn, Option<(String, String, bool)>);

    fn harness() -> Harness<'static, State> {
        let signin =
            SignIn::new("postgres://hydra-db.office.local:5433/hydra?sslmode=require".into())
                .unwrap();
        let mut h = theme::harness(
            egui::vec2(600.0, 560.0),
            |ui, (s, submitted): &mut State| {
                if let Some((c, remember)) = s.window(ui.ctx()) {
                    *submitted = Some((c.user, c.password, remember));
                }
            },
            (signin, None),
        );
        h.run();
        h
    }

    #[test]
    fn signs_in_once_a_user_name_is_entered() {
        let mut h = harness();
        h.get_by_label("Database hydra on hydra-db.office.local:5433");
        h.get_by_label("Remember me on this computer");
        assert!(h.get_by_label("Sign in").accesskit_node().is_disabled());
        assert!(h.query_by_label(WRONG_LOGIN).is_none());

        for (role, label, text) in [
            (Role::TextInput, "User name", " anna "),
            (Role::PasswordInput, "Password", "p@ss:w/rd"),
        ] {
            h.get_by_role_and_label(role, label).click();
            h.run();
            h.get_by_role_and_label(role, label).type_text(text);
            h.run();
        }
        h.run();
        assert!(!h.get_by_label("Sign in").accesskit_node().is_disabled());
        h.get_by_label("Sign in").click();
        h.run();
        assert_eq!(h.state().1, Some(("anna".into(), "p@ss:w/rd".into(), true)));
    }

    #[test]
    fn enter_in_the_password_field_signs_in() {
        let mut h = harness();
        h.state_mut().0.remember = false;
        h.state_mut().0.user = "anna".into();
        h.run();
        h.get_by_role_and_label(Role::PasswordInput, "Password")
            .click();
        h.run();
        h.get_by_role_and_label(Role::PasswordInput, "Password")
            .type_text("pw");
        h.run();
        assert_eq!(h.state().1, None, "typing alone doesn't sign in");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().1, Some(("anna".into(), "pw".into(), false)));
    }

    #[test]
    fn enter_without_a_user_name_does_nothing() {
        let mut h = harness();
        h.get_by_role_and_label(Role::PasswordInput, "Password")
            .click();
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().1, None);
    }

    #[test]
    fn says_when_the_password_was_wrong() {
        let mut h = harness();
        h.state_mut().0.wrong_login = true;
        h.run();
        h.get_by_label(WRONG_LOGIN);
    }

    #[test]
    fn retry_reconnects_after_a_connection_failure() {
        let mut signin = SignIn::new("postgres://127.0.0.1:1/none".into()).unwrap();
        signin.signed_in = Some(Credentials {
            user: "anna".into(),
            password: "pw".into(),
        });
        let mut shared = Shared::new(hydra_core::Catalogue::default());
        let (requests, _inbox) = std::sync::mpsc::channel();
        shared.db = Some(Db::new(requests, std::sync::mpsc::channel().1));
        let mut h = theme::harness(
            egui::vec2(600.0, 100.0),
            |ui, (s, shared): &mut (SignIn, Shared)| s.status(ui, shared),
            (signin, shared),
        );
        h.run();
        h.get_by_label("Signed in as anna");
        // The database's name, as in the app bar, with the server in its hover.
        h.get_by_label("none");
        assert!(h.query_by_label("Retry").is_none());

        h.state_mut().0.connect_failed();
        h.run();
        h.get_by_label("Retry").click();
        h.run();
        assert!(h.query_by_label("Retry").is_none());
        let (s, shared) = h.state();
        assert!(shared.db.is_some());
        assert_eq!(
            shared.retired.len(),
            1,
            "the failed worker's replies still arrive"
        );
        assert_eq!(s.signed_in.as_ref().map(|c| &c.password[..]), Some("pw"));
    }

    #[test]
    fn a_new_password_reconnects() {
        let mut s = SignIn::test("anna", "old-password-1");
        s.options = "postgres://127.0.0.1:1/none".parse().unwrap();
        let mut shared = Shared::new(hydra_core::Catalogue::default());
        let (requests, _inbox) = std::sync::mpsc::channel();
        shared.db = Some(Db::new(requests, std::sync::mpsc::channel().1));
        assert!(s.is_password("old-password-1"));
        s.password_changed(
            "new-password-1".into(),
            &mut shared,
            &egui::Context::default(),
        );
        assert!(s.is_password("new-password-1"));
        assert_eq!(s.user(), Some("anna"));
        assert!(shared.db.is_some());
        assert_eq!(shared.retired.len(), 1, "the old worker is retired");
    }

    #[test]
    fn a_keychain_that_refuses_the_new_password_forgets_the_old_one() {
        let c = Credentials {
            user: "anna".into(),
            password: "new-password-1".into(),
        };
        let forgotten = std::cell::Cell::new(false);
        assert!(!replace_stored(
            "u",
            &c,
            |_, _| false,
            |_| forgotten.set(true)
        ));
        assert!(forgotten.get());
        forgotten.set(false);
        assert!(replace_stored(
            "u",
            &c,
            |_, _| true,
            |_| forgotten.set(true)
        ));
        assert!(!forgotten.get());
    }
}
