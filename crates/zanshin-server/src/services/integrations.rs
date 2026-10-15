//! Outbound delivery integrations: webhook (HMAC-signed), OneSignal, Facebook app-to-user
//! notifications, and the server's own FCM / APNs push providers.

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    integrations::{Integration, IntegrationKind},
    properties::PropertyMap,
    Error, Result,
};
use zanshin_store::integrations::IntegrationRepo;

use super::push::PushService;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IntegrationPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<IntegrationKind>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// One rendered message bound for one player.
pub struct Outbound<'a> {
    pub message_key: &'a str,
    pub identity_id: Uuid,
    pub external_id: &'a str,
    pub occurrence: Option<chrono::DateTime<chrono::Utc>>,
    pub locale: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub image_url: &'a str,
    pub payload: &'a Value,
    pub props: &'a PropertyMap,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub channel: String,
    pub status: &'static str,
    pub detail: String,
}

impl Outcome {
    fn sent(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "sent",
            detail: detail.into(),
        }
    }
    fn failed(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "failed",
            detail: detail.into(),
        }
    }
    fn skipped(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "skipped",
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
pub struct IntegrationService {
    repo: IntegrationRepo,
    http: reqwest::Client,
    push: PushService,
}

impl IntegrationService {
    pub fn new(repo: IntegrationRepo, push: PushService) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { repo, http, push }
    }

    pub async fn list(&self) -> Result<Vec<Integration>> {
        Ok(self
            .repo
            .list()
            .await?
            .iter()
            .map(Integration::masked)
            .collect())
    }

    pub async fn get(&self, key: &str) -> Result<Integration> {
        Ok(self.repo.get(key).await?.masked())
    }

    pub async fn exists(&self, key: &str) -> Result<()> {
        self.repo.get(key).await.map(|_| ())
    }

    pub async fn create(&self, key: &str, p: IntegrationPatch) -> Result<Integration> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let i = Integration {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            kind: p.kind.ok_or_else(|| Error::invalid("kind is required"))?,
            config: p.config.unwrap_or_else(|| json!({})),
            enabled: p.enabled.unwrap_or(true),
            health: Default::default(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        i.validate()?;
        Ok(self.repo.create(&i).await?.masked())
    }

    pub async fn update(&self, key: &str, p: IntegrationPatch) -> Result<Integration> {
        let mut i = self.repo.get(key).await?;
        if let Some(v) = p.name {
            i.name = v;
        }
        if let Some(v) = p.kind {
            i.kind = v;
        }
        if let Some(v) = p.config {
            i.config = Integration::merge_config(&i.config, v);
        }
        if let Some(v) = p.enabled {
            i.enabled = v;
        }
        i.validate()?;
        Ok(self.repo.update(&i).await?.masked())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await
    }

    /// Deliver through the named integration. Never fails hard: every problem becomes an outcome.
    pub async fn deliver(&self, key: &str, out: &Outbound<'_>) -> Outcome {
        let o = self.deliver_inner(key, out).await;
        if o.status != "skipped" {
            if let Err(e) = self.repo.record_outcome(key, o.status, &o.detail).await {
                tracing::warn!(integration = key, error = %e, "integration health not recorded");
            }
        }
        o
    }

    async fn deliver_inner(&self, key: &str, out: &Outbound<'_>) -> Outcome {
        let i = match self.repo.get(key).await {
            Ok(i) => i,
            Err(e) => return Outcome::failed(key, format!("integration unavailable: {e}")),
        };
        if !i.enabled {
            return Outcome::skipped(key, "integration disabled");
        }
        match i.kind {
            IntegrationKind::Webhook => self.webhook(&i, out).await,
            IntegrationKind::Onesignal => self.onesignal(&i, out).await,
            IntegrationKind::Facebook => self.facebook(&i, out).await,
            IntegrationKind::Smtp
            | IntegrationKind::Ses
            | IntegrationKind::Sendgrid
            | IntegrationKind::Resend => self.email(&i, out).await,
            IntegrationKind::Fcm | IntegrationKind::Apns => {
                if !self.push.enabled() {
                    return Outcome::skipped(key, "no push provider configured on the server");
                }
                let mut data = serde_json::Map::new();
                data.insert(
                    "zanshin_message_key".into(),
                    Value::String(out.message_key.into()),
                );
                if let Some(o) = out.payload.as_object() {
                    for (k, v) in o {
                        data.insert(k.clone(), v.clone());
                    }
                }
                let n = zanshin_core::push::Notification {
                    title: out.title.to_string(),
                    body: out.body.to_string(),
                    image_url: out.image_url.to_string(),
                    data,

                    options: None,
                };
                match self.push.notify(out.identity_id, &n).await {
                    0 => Outcome::skipped(key, "no registered devices"),
                    n => Outcome::sent(key, format!("{n} device(s)")),
                }
            }
        }
    }

    fn envelope(out: &Outbound<'_>) -> Value {
        json!({
            "event": "message",
            "message_key": out.message_key,
            "identity_id": out.identity_id,
            "external_id": out.external_id,
            "occurrence": out.occurrence,
            "locale": out.locale,
            "title": out.title,
            "body": out.body,
            "image_url": out.image_url,
            "payload": out.payload,
            "sent_at": chrono::Utc::now(),
        })
    }

    async fn post_json(&self, key: &str, req: reqwest::RequestBuilder, body: Vec<u8>) -> Outcome {
        match req
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(res) => {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(300).collect();
                if status.is_success() {
                    Outcome::sent(key, format!("HTTP {}", status.as_u16()))
                } else {
                    Outcome::failed(key, format!("HTTP {}: {snippet}", status.as_u16()))
                }
            }
            Err(e) => Outcome::failed(key, format!("request failed: {e}")),
        }
    }

    async fn webhook(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let url = i.config["url"].as_str().unwrap_or_default();
        let body = serde_json::to_vec(&Self::envelope(out)).unwrap_or_default();
        let ts = chrono::Utc::now().timestamp().to_string();
        let mut req = self
            .http
            .post(url)
            .header("x-zanshin-event", "message")
            .header("x-zanshin-timestamp", &ts)
            .header("x-zanshin-integration", &i.key);
        if let Some(secret) = i.config["secret"].as_str().filter(|s| !s.is_empty()) {
            req = req.header("x-zanshin-signature", sign(secret, &ts, &body));
        }
        if let Some(h) = i.config["headers"].as_object() {
            for (k, v) in h {
                if let Some(v) = v.as_str() {
                    req = req.header(k.as_str(), v);
                }
            }
        }
        self.post_json(&i.key, req, body).await
    }

    async fn onesignal(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let url = i.config["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .unwrap_or("https://onesignal.com/api/v1/notifications");
        let api_key = i.config["api_key"].as_str().unwrap_or_default();
        let mut headings = serde_json::Map::new();
        headings.insert("en".into(), Value::String(out.title.into()));
        headings.insert(out.locale.into(), Value::String(out.title.into()));
        let mut contents = serde_json::Map::new();
        contents.insert("en".into(), Value::String(out.body.into()));
        contents.insert(out.locale.into(), Value::String(out.body.into()));
        let mut body = json!({
            "app_id": i.config["app_id"],
            "include_external_user_ids": [out.external_id],
            "channel_for_external_user_ids": "push",
            "headings": headings,
            "contents": contents,
            "data": { "zanshin_message_key": out.message_key, "payload": out.payload },
        });
        if !out.image_url.is_empty() {
            body["big_picture"] = Value::String(out.image_url.into());
        }
        let req = self
            .http
            .post(url)
            .header("authorization", format!("Basic {api_key}"));
        self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
            .await
    }

    async fn facebook(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let fb_id = ["custom.facebook_id", "default.facebook_id", "custom.fb_id"]
            .iter()
            .find_map(|k| {
                out.props
                    .get(*k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            });
        let Some(fb_id) = fb_id else {
            return Outcome::skipped(&i.key, "player has no facebook_id property");
        };
        let url = match i.config["url"].as_str().filter(|u| !u.is_empty()) {
            Some(u) => u.replace("{user_id}", fb_id),
            None => format!("https://graph.facebook.com/v19.0/{fb_id}/notifications"),
        };
        let template: String = if out.body.is_empty() {
            out.title
        } else {
            out.body
        }
        .chars()
        .take(180)
        .collect();
        let mut body = json!({
            "access_token": i.config["access_token"],
            "template": template,
        });
        if let Some(href) = out.payload.get("href").and_then(|v| v.as_str()) {
            body["href"] = Value::String(href.into());
        }
        let req = self.http.post(&url);
        self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
            .await
    }
}

/// `sha256=<hex>` over `"{timestamp}.{body}"`, the same scheme receivers verify with.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let out = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + out.len() * 2);
    hex.push_str("sha256=");
    for b in out {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Look a property up by flattened key (`default.email`) or nested path.
fn prop_str<'a>(props: &'a PropertyMap, key: &str) -> Option<&'a str> {
    if let Some(v) = props.get(key).and_then(|v| v.as_str()) {
        return Some(v);
    }
    let mut cur: &Value = &Value::Null;
    for (n, part) in key.split('.').enumerate() {
        cur = if n == 0 {
            props.get(part)?
        } else {
            cur.get(part)?
        };
    }
    cur.as_str()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Email rendering shared by every provider: subject = title, text + simple HTML body.
struct Email {
    to: String,
    from: String,
    reply_to: String,
    subject: String,
    text: String,
    html: String,
}

impl IntegrationService {
    fn compose(i: &Integration, out: &Outbound<'_>) -> Result<Email, String> {
        let to_prop = i.config["to_prop"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or("default.email");
        let to = prop_str(out.props, to_prop)
            .filter(|e| e.contains('@'))
            .ok_or_else(|| format!("player has no email in {to_prop}"))?;
        let from = i.config["from"].as_str().unwrap_or_default();
        if from.is_empty() {
            return Err("integration has no `from` address".into());
        }
        let mut html = format!("<p>{}</p>", html_escape(out.body).replace('\n', "<br>"));
        if !out.image_url.is_empty() {
            html.push_str(&format!(
                "<p><img src=\"{}\" alt=\"\" style=\"max-width:100%\"></p>",
                html_escape(out.image_url)
            ));
        }
        if let Some(cta) = out.payload.get("cta_url").and_then(|v| v.as_str()) {
            let label = out
                .payload
                .get("cta_label")
                .and_then(|v| v.as_str())
                .unwrap_or("Open");
            html.push_str(&format!(
                "<p><a href=\"{}\">{}</a></p>",
                html_escape(cta),
                html_escape(label)
            ));
        }
        Ok(Email {
            to: to.to_string(),
            from: from.to_string(),
            reply_to: i.config["reply_to"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            subject: out.title.to_string(),
            text: out.body.to_string(),
            html,
        })
    }

    async fn email(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let mail = match Self::compose(i, out) {
            Ok(m) => m,
            Err(e) => return Outcome::skipped(&i.key, e),
        };
        self.send_email(i, &mail, out.message_key, out.external_id)
            .await
    }

    /// Sends an arbitrary email through an email integration (reports, tests); `to` may hold
    /// several recipients, one outcome each.
    pub(crate) async fn email_raw(
        &self,
        key: &str,
        to: &[String],
        subject: &str,
        text: &str,
        html: &str,
    ) -> Result<Vec<Outcome>> {
        let i = self.repo.get(key).await?;
        if !matches!(
            i.kind,
            IntegrationKind::Smtp
                | IntegrationKind::Ses
                | IntegrationKind::Sendgrid
                | IntegrationKind::Resend
        ) {
            return Err(Error::invalid(format!(
                "integration {key:?} is not an email provider"
            )));
        }
        let from = i.config["from"].as_str().unwrap_or_default().to_string();
        if from.is_empty() {
            return Err(Error::invalid("integration has no `from` address"));
        }
        let mut out = Vec::with_capacity(to.len());
        for rcpt in to {
            let mail = Email {
                to: rcpt.clone(),
                from: from.clone(),
                reply_to: i.config["reply_to"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                subject: subject.to_string(),
                text: text.to_string(),
                html: html.to_string(),
            };
            out.push(self.send_email(&i, &mail, "report", "").await);
        }
        Ok(out)
    }

    async fn send_email(
        &self,
        i: &Integration,
        mail: &Email,
        tag_key: &str,
        tag_id: &str,
    ) -> Outcome {
        match i.kind {
            IntegrationKind::Sendgrid => {
                let mut body = json!({
                    "personalizations": [{ "to": [{ "email": &mail.to }] }],
                    "from": { "email": &mail.from },
                    "subject": &mail.subject,
                    "content": [
                        { "type": "text/plain", "value": &mail.text },
                        { "type": "text/html", "value": &mail.html }
                    ],
                    "custom_args": { "zanshin_message_key": tag_key, "zanshin_identity": tag_id }
                });
                if !mail.reply_to.is_empty() {
                    body["reply_to"] = json!({ "email": mail.reply_to });
                }
                let url = i.config["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .unwrap_or("https://api.sendgrid.com/v3/mail/send");
                let req = self
                    .http
                    .post(url)
                    .bearer_auth(i.config["api_key"].as_str().unwrap_or_default());
                self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
                    .await
            }
            IntegrationKind::Resend => {
                let mut body = json!({
                    "from": &mail.from, "to": [&mail.to], "subject": &mail.subject,
                    "text": &mail.text, "html": &mail.html,
                    "tags": [{ "name": "zanshin_message_key", "value": tag_key.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-', "_") }]
                });
                if !mail.reply_to.is_empty() {
                    body["reply_to"] = Value::String(mail.reply_to.clone());
                }
                let url = i.config["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .unwrap_or("https://api.resend.com/emails");
                let req = self
                    .http
                    .post(url)
                    .bearer_auth(i.config["api_key"].as_str().unwrap_or_default());
                self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
                    .await
            }
            IntegrationKind::Ses => {
                let region = i.config["region"].as_str().unwrap_or("us-east-1");
                let host = i.config["host"]
                    .as_str()
                    .filter(|h| !h.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("email.{region}.amazonaws.com"));
                let path = "/v2/email/outbound-emails";
                let mut body = json!({
                    "FromEmailAddress": &mail.from,
                    "Destination": { "ToAddresses": [&mail.to] },
                    "Content": { "Simple": {
                        "Subject": { "Data": &mail.subject, "Charset": "UTF-8" },
                        "Body": {
                            "Text": { "Data": &mail.text, "Charset": "UTF-8" },
                            "Html": { "Data": &mail.html, "Charset": "UTF-8" }
                        }
                    }}
                });
                if !mail.reply_to.is_empty() {
                    body["ReplyToAddresses"] = json!([mail.reply_to]);
                }
                let payload = serde_json::to_vec(&body).unwrap_or_default();
                let (auth, amz_date, payload_hash) = super::export::ExportService::sigv4(
                    "POST",
                    &host,
                    path,
                    "ses",
                    region,
                    i.config["access_key"].as_str().unwrap_or_default(),
                    i.config["secret_key"].as_str().unwrap_or_default(),
                    &[],
                    &payload,
                );
                let req = self
                    .http
                    .post(format!("https://{host}{path}"))
                    .header("authorization", auth)
                    .header("x-amz-date", amz_date)
                    .header("x-amz-content-sha256", payload_hash);
                self.post_json(&i.key, req, payload).await
            }
            _ => match smtp::send(&i.config, mail).await {
                Ok(id) => Outcome::sent(&i.key, id),
                Err(e) => Outcome::failed(&i.key, e),
            },
        }
    }
}

/// Minimal SMTP client (RFC 5321 + STARTTLS / implicit TLS + AUTH LOGIN); enough for SES SMTP,
/// Mailgun, Postmark, a self-hosted Postfix… without pulling in a mail crate.
mod smtp {
    use std::sync::Arc;

    use base64::Engine;
    use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    use super::Email;

    type Stream = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
    type Sink = Box<dyn AsyncWrite + Send + Unpin>;

    struct Conn {
        r: Stream,
        w: Sink,
    }

    impl Conn {
        fn new<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(s: S) -> Self {
            let (r, w) = tokio::io::split(s);
            Self {
                r: BufReader::new(Box::new(r)),
                w: Box::new(w),
            }
        }

        /// Read one (possibly multi-line) reply; Ok(text) when the code is 2xx/3xx.
        async fn reply(&mut self) -> Result<String, String> {
            let mut all = String::new();
            loop {
                let mut line = String::new();
                let n = self
                    .r
                    .read_line(&mut line)
                    .await
                    .map_err(|e| format!("read: {e}"))?;
                if n == 0 {
                    return Err("connection closed".into());
                }
                all.push_str(&line);
                if line.len() < 4 || line.as_bytes()[3] != b'-' {
                    break;
                }
            }
            let code: u16 = all.get(0..3).and_then(|c| c.parse().ok()).unwrap_or(0);
            if (200..400).contains(&code) {
                Ok(all.trim().to_string())
            } else {
                Err(all.trim().chars().take(300).collect())
            }
        }

        async fn cmd(&mut self, c: &str) -> Result<String, String> {
            self.w
                .write_all(c.as_bytes())
                .await
                .map_err(|e| format!("write: {e}"))?;
            self.w
                .write_all(b"\r\n")
                .await
                .map_err(|e| format!("write: {e}"))?;
            self.w.flush().await.map_err(|e| format!("write: {e}"))?;
            self.reply().await
        }
    }

    fn tls_connector() -> TlsConnector {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        TlsConnector::from(Arc::new(cfg))
    }

    fn dot_stuff(body: &str) -> String {
        body.lines()
            .map(|l| {
                if l.starts_with('.') {
                    format!(".{l}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    pub async fn send(cfg: &serde_json::Value, mail: &Email) -> Result<String, String> {
        let host = cfg["host"].as_str().unwrap_or_default();
        let port = cfg["port"].as_u64().unwrap_or(587) as u16;
        let mode = cfg["tls"]
            .as_str()
            .unwrap_or(if port == 465 { "implicit" } else { "starttls" });
        let user = cfg["username"].as_str().unwrap_or_default();
        let pass = cfg["password"].as_str().unwrap_or_default();
        let helo = cfg["helo"]
            .as_str()
            .filter(|h| !h.is_empty())
            .unwrap_or("zanshin");
        let fut = async {
            let tcp = TcpStream::connect((host, port))
                .await
                .map_err(|e| format!("connect {host}:{port}: {e}"))?;
            let server = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|e| format!("bad host: {e}"))?;
            let mut conn = if mode == "implicit" {
                Conn::new(
                    tls_connector()
                        .connect(server, tcp)
                        .await
                        .map_err(|e| format!("tls: {e}"))?,
                )
            } else {
                let mut c = Conn::new(tcp);
                c.reply().await?;
                let ehlo = c.cmd(&format!("EHLO {helo}")).await?;
                if mode == "none" {
                    c
                } else {
                    if !ehlo.to_ascii_uppercase().contains("STARTTLS") {
                        return Err(
                            "server does not offer STARTTLS (set tls = \"none\" to send in clear)"
                                .into(),
                        );
                    }
                    c.cmd("STARTTLS").await?;
                    // Re-join the halves: the plain stream is consumed into the TLS session.
                    let (r, w) = (c.r.into_inner(), c.w);
                    let plain = Rejoin { r, w };
                    Conn::new(
                        tls_connector()
                            .connect(server, plain)
                            .await
                            .map_err(|e| format!("tls: {e}"))?,
                    )
                }
            };
            if mode == "implicit" {
                conn.reply().await?;
            }
            if mode != "none" {
                conn.cmd(&format!("EHLO {helo}")).await?;
            }
            if !user.is_empty() {
                let b64 = base64::engine::general_purpose::STANDARD;
                conn.cmd("AUTH LOGIN").await?;
                conn.cmd(&b64.encode(user)).await?;
                conn.cmd(&b64.encode(pass))
                    .await
                    .map_err(|e| format!("auth: {e}"))?;
            }
            conn.cmd(&format!("MAIL FROM:<{}>", mail.from)).await?;
            conn.cmd(&format!("RCPT TO:<{}>", mail.to)).await?;
            conn.cmd("DATA").await?;
            let boundary = format!("zanshin-{}", uuid::Uuid::now_v7().simple());
            let mut msg = format!(
                "From: {}\r\nTo: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nDate: {}\r\nMessage-ID: <{}@zanshin>\r\n",
                mail.from, mail.to, mail.subject.replace(['\r', '\n'], " "), chrono::Utc::now().to_rfc2822(), uuid::Uuid::now_v7().simple()
            );
            if !mail.reply_to.is_empty() {
                msg.push_str(&format!("Reply-To: {}\r\n", mail.reply_to));
            }
            msg.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{}\r\n--{boundary}\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{}\r\n--{boundary}--\r\n",
                dot_stuff(&mail.text), dot_stuff(&mail.html)
            ));
            msg.push('.');
            let accepted = conn.cmd(&msg).await?;
            let _ = conn.cmd("QUIT").await;
            Ok(accepted)
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), fut)
            .await
            .map_err(|_| "smtp timed out".to_string())?
    }

    /// Glue a split read half + write half back into one duplex stream for the TLS handshake.
    struct Rejoin {
        r: Box<dyn AsyncRead + Send + Unpin>,
        w: Box<dyn AsyncWrite + Send + Unpin>,
    }

    impl AsyncRead for Rejoin {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.r).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Rejoin {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.w).poll_write(cx, buf)
        }
        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.w).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.w).poll_shutdown(cx)
        }
    }
}
