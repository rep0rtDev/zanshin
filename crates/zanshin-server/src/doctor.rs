//! `zanshin doctor`: one command that checks a deployment end to end and explains what is wrong
//! in plain words — configuration sanity, PostgreSQL + pending migrations, Redis, ClickHouse,
//! clock skew, configured integrations and SSO. Exit code 1 when anything FAILs.

use std::time::Instant;

use serde::Serialize;
use zanshin_store::Stores;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    pub ms: u64,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub version: &'static str,
    pub production: bool,
    pub checks: Vec<Check>,
    pub ok: bool,
}

fn check(name: &str, status: Status, detail: impl Into<String>, started: Instant) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
        ms: started.elapsed().as_millis() as u64,
    }
}

pub async fn run(cfg: &Config) -> Report {
    let mut checks = Vec::new();

    // ---- configuration sanity ------------------------------------------------------------------
    let t = Instant::now();
    let secret = cfg.auth.jwt_secret.len();
    checks.push(if secret < 32 {
        check(
            "auth.jwt_secret",
            Status::Fail,
            format!("{secret} chars — use at least 32 random characters (openssl rand -base64 48)"),
            t,
        )
    } else if cfg.auth.jwt_secret.to_lowercase().contains("secret")
        || cfg.auth.jwt_secret.contains("change")
    {
        check(
            "auth.jwt_secret",
            Status::Warn,
            "looks like a placeholder value",
            t,
        )
    } else {
        check("auth.jwt_secret", Status::Ok, format!("{secret} chars"), t)
    });
    let t = Instant::now();
    checks.push(match &cfg.admin.bootstrap_key {
        Some(k) if k.len() < 16 => check(
            "admin.bootstrap_key",
            Status::Fail,
            "shorter than 16 chars",
            t,
        ),
        Some(_) if cfg.server.production => check(
            "admin.bootstrap_key",
            Status::Warn,
            "set in production — create console users / API keys, then remove it from the config",
            t,
        ),
        Some(_) => check("admin.bootstrap_key", Status::Ok, "set", t),
        None => check(
            "admin.bootstrap_key",
            Status::Ok,
            "not set (console users / API keys in use)",
            t,
        ),
    });
    let t = Instant::now();
    checks.push(if cfg.server.production && !cfg.server.trust_proxy {
        check("server.trust_proxy", Status::Warn, "production without trust_proxy: client IPs and https detection will come from the proxy hop; enable it when behind Caddy / nginx / Coolify", t)
    } else {
        check("server.production", Status::Ok, if cfg.server.production { "production mode" } else { "development mode (not for real traffic)" }, t)
    });
    let t = Instant::now();
    let sso: Vec<&str> = [
        cfg.admin.oidc.as_ref().map(|_| "oidc"),
        cfg.admin.saml.as_ref().map(|_| "saml"),
        cfg.admin
            .scim
            .as_ref()
            .filter(|s| !s.token().is_empty())
            .map(|_| "scim"),
    ]
    .into_iter()
    .flatten()
    .collect();
    checks.push(check(
        "console.sso",
        Status::Ok,
        if sso.is_empty() {
            "password + API keys only".to_string()
        } else {
            sso.join(", ")
        },
        t,
    ));
    if let Some(saml) = &cfg.admin.saml {
        let t = Instant::now();
        let bad = saml
            .idp_certs
            .iter()
            .filter(|c| crate::services::saml::parse_cert(c).is_err())
            .count();
        checks.push(if saml.idp_certs.is_empty() || bad > 0 {
            check(
                "saml.idp_certs",
                Status::Fail,
                format!(
                    "{bad} of {} certificates unusable (PEM / base64 DER, RSA)",
                    saml.idp_certs.len()
                ),
                t,
            )
        } else {
            check(
                "saml.idp_certs",
                Status::Ok,
                format!("{} certificate(s) parse", saml.idp_certs.len()),
                t,
            )
        });
    }
    let t = Instant::now();
    checks.push(check(
        "push.provider",
        if cfg.push.provider == "log" && cfg.server.production {
            Status::Warn
        } else {
            Status::Ok
        },
        format!(
            "{}{}",
            cfg.push.provider,
            if cfg.push.provider == "log" {
                " (pushes are only logged)"
            } else {
                ""
            }
        ),
        t,
    ));

    // ---- stores --------------------------------------------------------------------------------
    let t = Instant::now();
    let stores = match Stores::connect(&cfg.database, &cfg.redis, &cfg.clickhouse).await {
        Ok(s) => s,
        Err(e) => {
            checks.push(check("stores.connect", Status::Fail, format!("{e:#}"), t));
            return finish(cfg, checks);
        }
    };
    let h = stores.health().await;
    checks.push(check(
        "postgres",
        if h.postgres.ok {
            Status::Ok
        } else {
            Status::Fail
        },
        format!(
            "{} (pool {} / idle {})",
            describe(&h.postgres),
            h.pool.size,
            h.pool.idle
        ),
        t,
    ));
    checks.push(check(
        "redis",
        if h.redis.ok { Status::Ok } else { Status::Fail },
        describe(&h.redis),
        t,
    ));
    match &h.clickhouse {
        Some(c) => checks.push(check(
            "clickhouse",
            if c.ok { Status::Ok } else { Status::Fail },
            describe(c),
            t,
        )),
        None => checks.push(check(
            "clickhouse",
            Status::Ok,
            "not configured — events live in PostgreSQL (fine up to ~tens of millions of events)",
            t,
        )),
    }
    let t = Instant::now();
    checks.push(match Stores::pending_migrations(&cfg.database).await {
        Ok(p) if p.is_empty() => {
            check("migrations", Status::Ok, "database schema is up to date", t)
        }
        Ok(p) => check(
            "migrations",
            if cfg.database.migrate {
                Status::Warn
            } else {
                Status::Fail
            },
            format!(
                "{} pending ({}){}",
                p.len(),
                p.join(", "),
                if cfg.database.migrate {
                    " — applied automatically on start"
                } else {
                    " — run `zanshin --migrate`"
                }
            ),
            t,
        ),
        Err(e) => check("migrations", Status::Fail, format!("{e:#}"), t),
    });
    let t = Instant::now();
    match stores.doctor_facts().await {
        Ok((db_now, rows, users)) => {
            let skew = (chrono::Utc::now() - db_now).num_milliseconds().abs();
            checks.push(check("clock", if skew > 5_000 { Status::Fail } else if skew > 1_000 { Status::Warn } else { Status::Ok }, format!("{skew} ms between this host and PostgreSQL (sessions, schedules and quiet hours depend on it)"), t));
            let disabled = rows.iter().filter(|r| !r.2).count();
            let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
            for r in &rows {
                *by_kind.entry(r.1.as_str()).or_default() += 1;
            }
            checks.push(check(
                "integrations",
                Status::Ok,
                if rows.is_empty() {
                    "none configured (push / email / webhooks will be no-ops)".to_string()
                } else {
                    format!(
                        "{} configured ({}){}",
                        rows.len(),
                        by_kind
                            .iter()
                            .map(|(k, n)| format!("{k} ×{n}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if disabled > 0 {
                            format!(", {disabled} disabled")
                        } else {
                            String::new()
                        }
                    )
                },
                t,
            ));
            checks.push(if users == 0 {
                check("console.users", if cfg.admin.bootstrap_key.is_some() { Status::Warn } else { Status::Fail }, "no console users yet — sign in with the bootstrap key once and create the first admin", t)
            } else {
                check("console.users", Status::Ok, format!("{users} active"), t)
            });
        }
        Err(e) => checks.push(check(
            "database.facts",
            Status::Warn,
            format!("could not read clock / integrations / users: {e}"),
            t,
        )),
    }
    finish(cfg, checks)
}

fn describe(c: &zanshin_store::Component) -> String {
    match (&c.error, c.ok) {
        (_, true) => format!("ok, {} ms", c.latency_ms),
        (Some(e), false) => e.clone(),
        (None, false) => "failed".into(),
    }
}

fn finish(cfg: &Config, checks: Vec<Check>) -> Report {
    let ok = !checks.iter().any(|c| c.status == Status::Fail);
    Report {
        version: env!("CARGO_PKG_VERSION"),
        production: cfg.server.production,
        checks,
        ok,
    }
}

pub fn render(r: &Report) -> String {
    let mut out = format!(
        "zanshin doctor — v{} ({})\n",
        r.version,
        if r.production {
            "production"
        } else {
            "development"
        }
    );
    for c in &r.checks {
        let tag = match c.status {
            Status::Ok => "OK  ",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        out.push_str(&format!(
            "{tag}  {:<20} {}  ({} ms)\n",
            c.name, c.detail, c.ms
        ));
    }
    out.push_str(if r.ok {
        "\nAll checks passed.\n"
    } else {
        "\nSome checks FAILED — fix them before serving traffic.\n"
    });
    out
}
