//! Installation settings from `hydra.ini`: the database address and the update check URL.

use std::path::PathBuf;

pub const FILE: &str = "hydra.ini";

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Config {
    /// Postgres URL without user name or password; each user signs in with their own role.
    pub database_url: Option<String>,
    /// https URL of the release version JSON (see `update.rs`).
    pub update_url: Option<String>,
}

impl Config {
    /// Apply the file's `key = value` lines over these settings. An empty value unsets the key.
    pub fn merge(&mut self, text: &str) {
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(['#', ';', '[']) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                tracing::warn!("{FILE}: ignoring line without '=': {line}");
                continue;
            };
            let value = Some(value.trim().to_string()).filter(|v| !v.is_empty());
            match key.trim() {
                "database_url" => self.database_url = value,
                "update_url" => self.update_url = value,
                other => tracing::warn!("{FILE}: ignoring unknown key {other:?}"),
            }
        }
    }
}

/// The installer's file, for every user of the machine.
pub fn machine_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("ProgramData").map(|d| PathBuf::from(d).join("Hydra").join(FILE))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Application Support/Hydra").join(FILE))
    } else {
        Some(PathBuf::from("/etc/hydra").join(FILE))
    }
}

/// Read the machine file. A missing file is fine; other read errors come back as messages for
/// the status line.
pub fn load() -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut problems = Vec::new();
    if let Some(path) = machine_path() {
        match std::fs::read_to_string(&path) {
            Ok(text) => config.merge(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => problems.push(format!("Could not read {}: {e}", path.display())),
        }
    }
    (config, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_and_skips_the_rest() {
        let mut c = Config::default();
        c.merge(
            "# written by the installer\n\
             [hydra]\n\
             ; comment\n\
             \n\
             database_url=postgres://db.office.local:5433/hydra?sslmode=require\n  \
             update_url =  https://example.com/hydra/latest.json  \r\n\
             colour = blue\n\
             no equals sign\n",
        );
        assert_eq!(
            c,
            Config {
                database_url: Some("postgres://db.office.local:5433/hydra?sslmode=require".into()),
                update_url: Some("https://example.com/hydra/latest.json".into()),
            }
        );
    }

    #[test]
    fn an_empty_value_unsets_the_key() {
        let mut c = Config::default();
        c.merge("database_url = postgres://machine/hydra\nupdate_url =");
        assert_eq!(c.database_url.as_deref(), Some("postgres://machine/hydra"));
        assert_eq!(c.update_url, None);
    }
}
