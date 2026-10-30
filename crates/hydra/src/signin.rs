//! Signing in to the shared database with the user's own Postgres role, so revisions record
//! who made them. A remembered password lives in the OS keychain, one entry per database.

use eframe::egui;
use hydra_db::PgConnectOptions;

use crate::catalogue::Shared;
use crate::config::FILE;
use crate::db::{Db, WRONG_LOGIN};

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
            connect_failed: false,
        })
    }

    /// Connect with the credentials remembered for this database, if any.
    pub fn start(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        if let Some(c) = load(&self.url) {
            self.connect(c, false, shared, ctx);
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
            store(&self.url, c);
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

    fn sign_out(&mut self, shared: &mut Shared) {
        forget(&self.url);
        retire(shared);
        self.signed_in = None;
        self.to_remember = false;
        self.password.clear();
        self.open = true;
    }

    /// "Signed in as …" and a Sign out button, for the status line, plus Retry after a
    /// connection failure.
    pub fn status(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let Some(c) = &self.signed_in else {
            return;
        };
        ui.weak(format!("Signed in as {}", c.user));
        if ui.small_button("Sign out").clicked() {
            self.sign_out(shared);
        } else if self.connect_failed
            && ui.small_button("Retry").clicked()
            && let Some(c) = self.signed_in.take()
        {
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
        egui::Modal::new(egui::Id::new("sign-in")).show(ctx, |ui| {
            ui.heading("Sign in to Hydra");
            let o = &self.options;
            ui.weak(format!(
                "Database {} on {}:{}",
                o.get_database().unwrap_or("hydra"),
                o.get_host(),
                o.get_port()
            ));
            ui.add_space(4.0);
            egui::Grid::new("sign-in-fields")
                .num_columns(2)
                .show(ui, |ui| {
                    let label = ui.label("User name");
                    ui.add(egui::TextEdit::singleline(&mut self.user))
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label("Password");
                    let password = ui
                        .add(egui::TextEdit::singleline(&mut self.password).password(true))
                        .labelled_by(label.id);
                    submit = password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                });
            ui.checkbox(&mut self.remember, "Remember me on this computer");
            if self.wrong_login {
                ui.colored_label(ui.visuals().error_fg_color, WRONG_LOGIN);
            }
            let ready = !self.user.trim().is_empty();
            let clicked = ui
                .add_enabled(ready, egui::Button::new("Sign in"))
                .clicked();
            submit = ready && (submit || clicked);
        });
        submit.then(|| {
            let c = Credentials {
                user: self.user.trim().to_string(),
                password: self.password.clone(),
            };
            (c, self.remember)
        })
    }
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

fn store(url: &str, c: &Credentials) {
    let secret = serde_json::json!({"user": c.user, "password": c.password}).to_string();
    if let Err(e) = keyring::Entry::new(SERVICE, url).and_then(|e| e.set_password(&secret)) {
        tracing::warn!("keychain: {e}");
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
        let mut h = Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(
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
        let mut h = Harness::builder()
            .with_size(egui::vec2(600.0, 100.0))
            .build_ui_state(
                |ui, (s, shared): &mut (SignIn, Shared)| s.status(ui, shared),
                (signin, shared),
            );
        h.run();
        h.get_by_label("Signed in as anna");
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
}
