//! Startup check for a newer release, against the `update_url` of `hydra.ini`.
//!
//! The URL serves `{"version": "1.2.3", "windows": "https://…msi", "macos": "https://…dmg",
//! "notes": "optional"}`.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use ureq::tls::{RootCerts, TlsConfig};

#[derive(Debug, PartialEq)]
pub struct Update {
    pub version: String,
    /// This platform's installer.
    pub url: String,
    pub notes: Option<String>,
}

/// This platform's key in the version JSON.
const PLATFORM: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/// Fetch the version JSON on a background thread. The receiver gets an [`Update`] only when
/// a newer release has an installer for this platform; failures are only logged.
pub fn check(url: String, ctx: egui::Context) -> Receiver<Update> {
    let (tx, rx) = channel();
    std::thread::spawn(move || match fetch(&url) {
        Ok(json) => match newer(&json, env!("CARGO_PKG_VERSION"), PLATFORM) {
            Ok(Some(update)) => {
                let _ = tx.send(update);
                ctx.request_repaint();
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("update check: {url}: {e}"),
        },
        Err(e) => tracing::warn!("update check: {url}: {e}"),
    });
    rx
}

fn fetch(url: &str) -> Result<String, String> {
    if !url.starts_with("https://") {
        return Err("update_url must be an https URL".into());
    }
    // The OS trust store, so a server with a company CA certificate works.
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .tls_config(tls)
        .build();
    ureq::Agent::new_with_config(config)
        .get(url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| e.to_string())
}

/// The release in `json`, if it is newer than `current` and has an https installer for
/// `platform`.
pub fn newer(json: &str, current: &str, platform: &str) -> Result<Option<Update>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let version = v["version"].as_str().ok_or("no \"version\"")?;
    let (Some(latest), Some(current)) = (numeric(version), numeric(current)) else {
        return Err(format!(
            "can't compare versions {version:?} and {current:?}"
        ));
    };
    if latest <= current {
        return Ok(None);
    }
    let Some(url) = v[platform].as_str() else {
        return Ok(None);
    };
    if !url.starts_with("https://") {
        return Err(format!("installer URL is not https: {url}"));
    }
    Ok(Some(Update {
        version: version.to_string(),
        url: url.to_string(),
        notes: v["notes"].as_str().map(str::to_string),
    }))
}

/// `major.minor.patch` as numbers; anything else (pre-release tags, missing parts) is `None`.
fn numeric(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically() {
        assert_eq!(numeric("1.10.0"), Some((1, 10, 0)));
        assert!(numeric("1.10.0") > numeric("1.9.9"));
        for bad in ["1.2", "1.2.3.4", "1.2.3-beta", "v1.2.3", ""] {
            assert_eq!(numeric(bad), None, "{bad}");
        }
    }

    #[test]
    fn offers_only_a_newer_release_for_this_platform() {
        let json = r#"{"version": "1.10.0", "windows": "https://x/Hydra-1.10.0-x64.msi",
            "macos": "https://x/Hydra-1.10.0.dmg", "notes": "Faster saves."}"#;
        assert_eq!(
            newer(json, "1.9.3", "macos"),
            Ok(Some(Update {
                version: "1.10.0".into(),
                url: "https://x/Hydra-1.10.0.dmg".into(),
                notes: Some("Faster saves.".into()),
            }))
        );
        assert_eq!(newer(json, "1.10.0", "windows"), Ok(None));
        assert_eq!(newer(json, "2.0.0", "windows"), Ok(None));
        assert_eq!(newer(json, "0.1.0", "linux"), Ok(None));
        assert!(
            newer(
                r#"{"version": "2.0.0", "macos": "http://x/h.dmg"}"#,
                "1.0.0",
                "macos"
            )
            .is_err()
        );
        assert!(newer(r#"{"version": "2.0"}"#, "1.0.0", "macos").is_err());
        assert!(newer("not json", "1.0.0", "macos").is_err());
    }
}
