//! Release check: asks a GitHub-Releases-shaped endpoint for the latest version and remembers the
//! answer so the console can show an "update available" banner. Off with `updates.check = false`;
//! nothing leaves the server then.

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use chrono::{DateTime, Utc};

use crate::config::UpdatesConfig;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub enabled: bool,
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub url: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub struct UpdateChecker {
    cfg: UpdatesConfig,
    http: reqwest::Client,
    current: Option<semver::Version>,
    state: RwLock<Status>,
}

impl UpdateChecker {
    pub fn new(cfg: &UpdatesConfig) -> Arc<Self> {
        let current = crate::build_info::VERSION.to_string();
        Arc::new(Self {
            cfg: cfg.clone(),
            http: reqwest::Client::builder()
                .user_agent(format!("zanshin/{current}"))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            current: parse_version(&current),
            state: RwLock::new(Status {
                enabled: cfg.check,
                current,
                ..Default::default()
            }),
        })
    }

    pub fn status(&self) -> Status {
        self.state.read().expect("updates lock").clone()
    }

    /// True when the check is on and the last answer is older than `updates.interval` (or absent).
    pub fn due(&self) -> bool {
        if !self.cfg.check {
            return false;
        }
        let s = self.state.read().expect("updates lock");
        match s.checked_at {
            None => true,
            Some(at) => {
                Utc::now() - at > chrono::Duration::from_std(self.cfg.interval).unwrap_or_default()
            }
        }
    }

    pub async fn refresh_if_due(&self) {
        if self.due() {
            self.check().await;
        }
    }

    pub async fn check(&self) {
        if !self.cfg.check {
            return;
        }
        let now = Utc::now();
        let outcome = self.fetch().await;
        let mut s = self.state.write().expect("updates lock");
        s.checked_at = Some(now);
        match outcome {
            Ok(Some(r)) => {
                let latest = parse_version(&r.tag_name);
                s.update_available = match (&self.current, &latest) {
                    (Some(c), Some(l)) => l > c,
                    _ => false,
                };
                s.latest = Some(
                    latest
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| r.tag_name.clone()),
                );
                s.url = r.html_url;
                s.published_at = r.published_at;
                s.error = None;
                if s.update_available {
                    tracing::info!(current = %s.current, latest = ?s.latest, "a newer Zanshin release is available");
                }
            }
            Ok(None) => {
                s.update_available = false;
                s.latest = None;
                s.error = None;
            }
            Err(err) => {
                tracing::debug!(error = %err, "update check failed");
                s.error = Some(err.to_string());
            }
        }
    }

    /// Latest stable release, or `None` when the endpoint only has drafts / pre-releases.
    async fn fetch(&self) -> anyhow::Result<Option<Release>> {
        let resp = self
            .http
            .get(&self.cfg.url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?;
        let body: serde_json::Value = resp.json().await?;
        let releases: Vec<Release> = match body {
            serde_json::Value::Array(items) => items
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            other => vec![serde_json::from_value(other)?],
        };
        Ok(releases
            .into_iter()
            .filter(|r| !r.draft && !r.prerelease)
            .filter(|r| parse_version(&r.tag_name).is_some_and(|v| v.pre.is_empty()))
            .max_by_key(|r| parse_version(&r.tag_name)))
    }
}

fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches('v')).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_with_and_without_v_prefix() {
        assert_eq!(
            parse_version("v0.2.2").unwrap(),
            semver::Version::new(0, 2, 2)
        );
        assert_eq!(
            parse_version("1.0.0").unwrap(),
            semver::Version::new(1, 0, 0)
        );
        assert!(parse_version("nightly").is_none());
        assert!(parse_version("v0.3.0") > parse_version("v0.2.9"));
        assert!(parse_version("v0.3.0-rc.1") < parse_version("v0.3.0"));
    }
}
