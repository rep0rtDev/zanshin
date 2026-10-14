//! `zanshinctl`: operate a Zanshin server from the terminal / CI.
//!
//!   export ZANSHIN_URL=https://liveops.example.com ZANSHIN_ADMIN_KEY=zk_a_...
//!   zanshinctl config export --kinds flags,audiences > config.json
//!   zanshinctl config diff config.json --mode replace
//!   zanshinctl config promote --to https://prod.example.com --to-key zk_a_... --kinds flags
//!   zanshinctl player get account-42
//!   zanshinctl flags set new_shop --default-value true

use std::{io::Read, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "zanshinctl", version, about = "Zanshin admin CLI", long_about = None)]
struct Cli {
    /// Server URL (env ZANSHIN_URL)
    #[arg(long, env = "ZANSHIN_URL", global = true)]
    url: Option<String>,
    /// Admin API key (env ZANSHIN_ADMIN_KEY)
    #[arg(long, env = "ZANSHIN_ADMIN_KEY", global = true, hide_env_values = true)]
    key: Option<String>,
    /// Project slug for multi-project deployments (env ZANSHIN_PROJECT; default = `default`)
    #[arg(long, env = "ZANSHIN_PROJECT", global = true)]
    project: Option<String>,
    /// Compact JSON output (default: pretty)
    #[arg(long, global = true)]
    compact: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Server build / health info
    System,
    /// Configuration bundles: export, diff, import, promote between servers
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Admin / server / client API keys
    Keys {
        #[command(subcommand)]
        cmd: KeysCmd,
    },
    /// Players
    Player {
        #[command(subcommand)]
        cmd: PlayerCmd,
    },
    /// Feature flags
    Flags {
        #[command(subcommand)]
        cmd: FlagsCmd,
    },
    /// Export sinks
    Sinks {
        #[command(subcommand)]
        cmd: SinksCmd,
    },
    /// Raw admin API call: `zanshinctl api GET /admin/v1/flags`
    Api {
        method: String,
        path: String,
        /// JSON body (or @file, or - for stdin)
        body: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Merge,
    Replace,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Merge => "merge",
            Mode::Replace => "replace",
        }
    }
}

#[derive(Args)]
struct Kinds {
    /// Entity kinds, comma-separated (default: all)
    #[arg(long, value_delimiter = ',')]
    kinds: Vec<String>,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Entity kinds and bundle format
    Kinds,
    /// Export a bundle to stdout
    Export {
        #[command(flatten)]
        kinds: Kinds,
    },
    /// Compare a bundle file against the server
    Diff {
        /// Bundle file (- for stdin)
        file: PathBuf,
        #[arg(long, value_enum, default_value = "merge")]
        mode: Mode,
        #[command(flatten)]
        kinds: Kinds,
    },
    /// Import a bundle file
    Import {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "merge")]
        mode: Mode,
        #[command(flatten)]
        kinds: Kinds,
        /// Plan only, change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Export from this server and import into another one (dev → staging → prod)
    Promote {
        /// Target server URL
        #[arg(long)]
        to: String,
        /// Target project slug (default: same as --project)
        #[arg(long)]
        to_project: Option<String>,
        /// Target admin key (env ZANSHIN_TO_KEY)
        #[arg(long, env = "ZANSHIN_TO_KEY", hide_env_values = true)]
        to_key: String,
        #[arg(long, value_enum, default_value = "merge")]
        mode: Mode,
        #[command(flatten)]
        kinds: Kinds,
        #[arg(long)]
        dry_run: bool,
    },
    /// Change history (`--kind`, `--key`)
    History {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Revert an entity to a history snapshot
    Revert { snapshot_id: String },
}

#[derive(Subcommand)]
enum KeysCmd {
    List,
    /// Create a key: `--kind client|server|admin`
    Create {
        name: String,
        #[arg(long, default_value = "client")]
        kind: String,
        /// Permissions for admin keys, e.g. flags:read,flags:write
        #[arg(long, value_delimiter = ',')]
        permissions: Vec<String>,
    },
    /// Rotate a key (old secret keeps working for the grace period)
    Rotate {
        id: String,
        #[arg(long, default_value_t = 3600)]
        grace_secs: u64,
    },
    Revoke {
        id: String,
    },
}

#[derive(Subcommand)]
enum PlayerCmd {
    /// Look a player up by external id
    Get { external_id: String },
    /// Recent events of a player (`--name`, `--q`)
    Events {
        external_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Evaluate flags as the player would see them
    Flags {
        external_id: String,
        #[arg(long, value_delimiter = ',')]
        names: Vec<String>,
    },
    /// Delete a player and all attached data (GDPR)
    Delete {
        external_id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum FlagsCmd {
    List,
    Get {
        key: String,
    },
    /// Create or update a flag
    Set {
        key: String,
        #[arg(long)]
        value_type: Option<String>,
        /// JSON value (strings without quotes are accepted)
        #[arg(long)]
        default_value: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    Delete {
        key: String,
    },
}

#[derive(Subcommand)]
enum SinksCmd {
    List,
    /// Run a sink now
    Run {
        key: String,
    },
    /// Recent runs of a sink
    Runs {
        key: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Send a test batch
    Test {
        key: String,
    },
}

struct Api {
    base: String,
    key: String,
    project: Option<String>,
    http: reqwest::Client,
}

impl Api {
    fn new(url: &str, key: &str, project: Option<String>) -> Result<Self> {
        Ok(Self {
            base: url.trim_end_matches('/').to_string(),
            key: key.to_string(),
            project: project.filter(|p| !p.is_empty()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .user_agent(concat!("zanshinctl/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let m: reqwest::Method = method.to_uppercase().parse().context("bad HTTP method")?;
        let mut req = self
            .http
            .request(m, format!("{}{}", self.base, path))
            .bearer_auth(&self.key)
            .header("accept", "application/json");
        if let Some(p) = &self.project {
            req = req.header("x-zanshin-project", p);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req
            .send()
            .await
            .with_context(|| format!("{method} {path}"))?;
        let status = res.status();
        let text = res.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
        if !status.is_success() {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(300).collect());
            bail!("{method} {path} → {status}: {msg}");
        }
        Ok(v)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.call("GET", path, None).await
    }
    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call("POST", path, Some(body)).await
    }
}

fn read_json(file: &PathBuf) -> Result<Value> {
    let mut text = String::new();
    if file.as_os_str() == "-" {
        std::io::stdin().read_to_string(&mut text)?;
    } else {
        text = std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    }
    serde_json::from_str(&text).with_context(|| format!("{} is not JSON", file.display()))
}

fn parse_body(s: &str) -> Result<Value> {
    if let Some(f) = s.strip_prefix('@') {
        return read_json(&PathBuf::from(f));
    }
    if s == "-" {
        return read_json(&PathBuf::from("-"));
    }
    serde_json::from_str(s).context("body is not JSON")
}

fn loose_json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn kinds_q(k: &Kinds) -> String {
    if k.kinds.is_empty() {
        String::new()
    } else {
        format!("?kinds={}", enc(&k.kinds.join(",")))
    }
}

fn kinds_v(k: &Kinds) -> Value {
    if k.kinds.is_empty() {
        Value::Null
    } else {
        json!(k.kinds)
    }
}

fn print_plan(v: &Value) {
    if let Some(kinds) = v.get("plan").or(Some(v)).and_then(|p| p.as_object()) {
        for (kind, d) in kinds {
            let n = |f: &str| {
                d.get(f)
                    .and_then(|x| x.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0)
            };
            if d.is_object() && (d.get("added").is_some() || d.get("changed").is_some()) {
                eprintln!(
                    "{kind:<14} +{} ~{} -{} ={}",
                    n("added"),
                    n("changed"),
                    n("removed"),
                    n("unchanged")
                );
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let url = cli
        .url
        .clone()
        .context("server URL missing: --url or ZANSHIN_URL")?;
    let key = cli
        .key
        .clone()
        .context("admin key missing: --key or ZANSHIN_ADMIN_KEY")?;
    let api = Api::new(&url, &key, cli.project.clone())?;
    let out = match cli.cmd {
        Cmd::System => api.get("/admin/v1/system").await?,
        Cmd::Api { method, path, body } => {
            let b = body.as_deref().map(parse_body).transpose()?;
            api.call(&method, &path, b).await?
        }
        Cmd::Config { cmd } => match cmd {
            ConfigCmd::Kinds => api.get("/admin/v1/config/kinds").await?,
            ConfigCmd::Export { kinds } => {
                api.get(&format!("/admin/v1/config/export{}", kinds_q(&kinds)))
                    .await?
            }
            ConfigCmd::Diff { file, mode, kinds } => {
                let bundle = read_json(&file)?;
                let v = api
                    .post(
                        "/admin/v1/config/diff",
                        json!({"bundle": bundle, "mode": mode.as_str(), "kinds": kinds_v(&kinds)}),
                    )
                    .await?;
                print_plan(&v);
                v
            }
            ConfigCmd::Import {
                file,
                mode,
                kinds,
                dry_run,
            } => {
                let bundle = read_json(&file)?;
                let v = api
                    .post(
                        "/admin/v1/config/import",
                        json!({"bundle": bundle, "mode": mode.as_str(), "kinds": kinds_v(&kinds), "dry_run": dry_run}),
                    )
                    .await?;
                print_plan(&v);
                v
            }
            ConfigCmd::Promote {
                to,
                to_project,
                to_key,
                mode,
                kinds,
                dry_run,
            } => {
                let bundle = api
                    .get(&format!("/admin/v1/config/export{}", kinds_q(&kinds)))
                    .await?;
                let target = Api::new(&to, &to_key, to_project.or_else(|| cli.project.clone()))?;
                let v = target
                    .post(
                        "/admin/v1/config/import",
                        json!({"bundle": bundle, "mode": mode.as_str(), "kinds": kinds_v(&kinds), "dry_run": dry_run}),
                    )
                    .await?;
                eprintln!(
                    "{} {} → {}",
                    if dry_run { "planned" } else { "promoted" },
                    url.trim_end_matches('/'),
                    to.trim_end_matches('/')
                );
                print_plan(&v);
                v
            }
            ConfigCmd::History { kind, key, limit } => {
                let mut q = vec![format!("limit={limit}")];
                if let Some(k) = kind {
                    q.push(format!("kind={}", enc(&k)));
                }
                if let Some(k) = key {
                    q.push(format!("key={}", enc(&k)));
                }
                api.get(&format!("/admin/v1/config/history?{}", q.join("&")))
                    .await?
            }
            ConfigCmd::Revert { snapshot_id } => {
                api.post(
                    &format!("/admin/v1/config/history/{}/revert", enc(&snapshot_id)),
                    json!({}),
                )
                .await?
            }
        },
        Cmd::Keys { cmd } => match cmd {
            KeysCmd::List => api.get("/admin/v1/api-keys").await?,
            KeysCmd::Create {
                name,
                kind,
                permissions,
            } => {
                let mut b = json!({"name": name, "kind": kind});
                if !permissions.is_empty() {
                    b["permissions"] = json!(permissions);
                }
                api.post("/admin/v1/api-keys", b).await?
            }
            KeysCmd::Rotate { id, grace_secs } => {
                api.post(
                    &format!("/admin/v1/api-keys/{}/rotate", enc(&id)),
                    json!({"grace_secs": grace_secs}),
                )
                .await?
            }
            KeysCmd::Revoke { id } => {
                api.call("DELETE", &format!("/admin/v1/api-keys/{}", enc(&id)), None)
                    .await?
            }
        },
        Cmd::Player { cmd } => match cmd {
            PlayerCmd::Get { external_id } => {
                api.get(&format!(
                    "/admin/v1/identities/by-external-id/{}",
                    enc(&external_id)
                ))
                .await?
            }
            PlayerCmd::Events {
                external_id,
                name,
                q,
                limit,
            } => {
                let id = api
                    .get(&format!(
                        "/admin/v1/identities/by-external-id/{}",
                        enc(&external_id)
                    ))
                    .await?;
                let iid = id["id"].as_str().context("identity has no id")?.to_string();
                let mut qs = vec![format!("limit={limit}")];
                if let Some(n) = name {
                    qs.push(format!("name={}", enc(&n)));
                }
                if let Some(s) = q {
                    qs.push(format!("q={}", enc(&s)));
                }
                api.get(&format!(
                    "/admin/v1/identities/{iid}/events?{}",
                    qs.join("&")
                ))
                .await?
            }
            PlayerCmd::Flags { external_id, names } => {
                let mut p = format!("/server/v1/flags?external_id={}", enc(&external_id));
                if !names.is_empty() {
                    p += &format!("&names={}", enc(&names.join(",")));
                }
                api.get(&p).await?
            }
            PlayerCmd::Delete { external_id, yes } => {
                if !yes {
                    bail!("refusing to delete without --yes");
                }
                let id = api
                    .get(&format!(
                        "/admin/v1/identities/by-external-id/{}",
                        enc(&external_id)
                    ))
                    .await?;
                let iid = id["id"].as_str().context("identity has no id")?.to_string();
                api.call("DELETE", &format!("/admin/v1/data/identities/{iid}"), None)
                    .await?
            }
        },
        Cmd::Flags { cmd } => match cmd {
            FlagsCmd::List => api.get("/admin/v1/flags").await?,
            FlagsCmd::Get { key } => api.get(&format!("/admin/v1/flags/{}", enc(&key))).await?,
            FlagsCmd::Set {
                key,
                value_type,
                default_value,
                enabled,
                name,
                description,
            } => {
                let mut patch = serde_json::Map::new();
                if let Some(v) = value_type {
                    patch.insert("value_type".into(), json!(v));
                }
                if let Some(v) = default_value {
                    patch.insert("default_value".into(), loose_json(&v));
                }
                if let Some(v) = enabled {
                    patch.insert("enabled".into(), json!(v));
                }
                if let Some(v) = name {
                    patch.insert("name".into(), json!(v));
                }
                if let Some(v) = description {
                    patch.insert("description".into(), json!(v));
                }
                let exists = api
                    .get(&format!("/admin/v1/flags/{}", enc(&key)))
                    .await
                    .is_ok();
                if exists {
                    api.call(
                        "PUT",
                        &format!("/admin/v1/flags/{}", enc(&key)),
                        Some(Value::Object(patch)),
                    )
                    .await?
                } else {
                    patch.insert("key".into(), json!(key));
                    api.post("/admin/v1/flags", Value::Object(patch)).await?
                }
            }
            FlagsCmd::Delete { key } => {
                api.call("DELETE", &format!("/admin/v1/flags/{}", enc(&key)), None)
                    .await?
            }
        },
        Cmd::Sinks { cmd } => match cmd {
            SinksCmd::List => api.get("/admin/v1/export-sinks").await?,
            SinksCmd::Run { key } => {
                api.post(
                    &format!("/admin/v1/export-sinks/{}/run", enc(&key)),
                    json!({}),
                )
                .await?
            }
            SinksCmd::Runs { key, limit } => {
                api.get(&format!(
                    "/admin/v1/export-sinks/{}/runs?limit={limit}",
                    enc(&key)
                ))
                .await?
            }
            SinksCmd::Test { key } => {
                api.post(
                    &format!("/admin/v1/export-sinks/{}/test", enc(&key)),
                    json!({}),
                )
                .await?
            }
        },
    };
    if cli.compact {
        println!("{out}");
    } else {
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(())
}
