//! Event export sinks: periodic delivery of new events to S3, an HTTP endpoint, or BigQuery.

use std::io::Write;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zanshin_core::{
    export::{ExportFormat, ExportRun, ExportSink, SinkKind},
    flags::validate_key,
    Error, Result,
};
use zanshin_store::{
    events::ExportedEvent,
    export::{ExportRunRepo, ExportSinkRepo},
};

use super::{events::EventService, flags::normalize_tags};

const MASK: &str = "***";
const SECRET_KEYS: &[&str] = &[
    "secret_key",
    "secret",
    "private_key",
    "token",
    "password",
    "authorization",
];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SinkPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<SinkKind>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub format: Option<ExportFormat>,
    #[serde(default)]
    pub gzip: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub interval_secs: Option<i64>,
    #[serde(default)]
    pub batch_size: Option<i32>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct ExportService {
    sinks: ExportSinkRepo,
    runs: ExportRunRepo,
    events: EventService,
    http: reqwest::Client,
}

pub fn mask_config(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    if SECRET_KEYS.iter().any(|s| k.to_lowercase().contains(s)) && v.is_string() {
                        (k.clone(), Value::String(MASK.into()))
                    } else {
                        (k.clone(), mask_config(v))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Replace masked placeholders in `new` with the values from `old` at the same path.
fn unmask(new: Value, old: &Value) -> Value {
    match new {
        Value::String(s) if s == MASK => old.clone(),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| {
                    let prev = old.get(&k).cloned().unwrap_or(Value::Null);
                    (k, unmask(v, &prev))
                })
                .collect(),
        ),
        other => other,
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl ExportService {
    pub fn new(sinks: ExportSinkRepo, runs: ExportRunRepo, events: EventService) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            sinks,
            runs,
            events,
            http,
        }
    }

    pub async fn list(&self) -> Result<Vec<ExportSink>> {
        Ok(self
            .sinks
            .list()
            .await?
            .into_iter()
            .map(Self::masked)
            .collect())
    }

    pub async fn get(&self, key: &str) -> Result<ExportSink> {
        Ok(Self::masked(self.sinks.get(key).await?))
    }

    fn masked(mut s: ExportSink) -> ExportSink {
        s.config = mask_config(&s.config);
        s
    }

    pub async fn create(&self, key: &str, p: SinkPatch) -> Result<ExportSink> {
        validate_key(key)?;
        let now = Utc::now();
        let s = ExportSink {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            kind: p.kind.ok_or_else(|| Error::invalid("kind is required"))?,
            config: p.config.unwrap_or_else(|| json!({})),
            format: p.format.unwrap_or_default(),
            gzip: p.gzip.unwrap_or(true),
            enabled: p.enabled.unwrap_or(true),
            events: p.events.unwrap_or_default(),
            interval_secs: p.interval_secs.unwrap_or(300),
            batch_size: p.batch_size.unwrap_or(10_000),
            cursor_at: None,
            cursor_id: None,
            next_run_at: None,
            last_run_at: None,
            last_error: String::new(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        s.validate()?;
        Ok(Self::masked(self.sinks.create(&s).await?))
    }

    pub async fn update(&self, key: &str, p: SinkPatch) -> Result<ExportSink> {
        let mut s = self.sinks.get(key).await?;
        if let Some(v) = p.name {
            s.name = v;
        }
        if let Some(v) = p.kind {
            s.kind = v;
        }
        if let Some(v) = p.config {
            s.config = unmask(v, &s.config);
        }
        if let Some(v) = p.format {
            s.format = v;
        }
        if let Some(v) = p.gzip {
            s.gzip = v;
        }
        if let Some(v) = p.enabled {
            s.enabled = v;
        }
        if let Some(v) = p.events {
            s.events = v;
        }
        if let Some(v) = p.interval_secs {
            s.interval_secs = v;
        }
        if let Some(v) = p.batch_size {
            s.batch_size = v;
        }
        if let Some(v) = p.tags {
            s.tags = normalize_tags(v)?;
        }
        s.validate()?;
        Ok(Self::masked(self.sinks.update(&s).await?))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.sinks.delete(key).await
    }

    pub async fn runs(&self, key: &str, limit: i64) -> Result<Vec<ExportRun>> {
        let s = self.sinks.get(key).await?;
        self.runs.list(s.id, limit.clamp(1, 500)).await
    }

    pub async fn trigger(&self, key: &str) -> Result<()> {
        self.sinks.get(key).await?;
        self.sinks.trigger(key).await
    }

    pub async fn reset_cursor(&self, key: &str, to: Option<DateTime<Utc>>) -> Result<()> {
        self.sinks.reset_cursor(key, to).await
    }

    pub async fn purge_runs(&self, keep: chrono::Duration) -> Result<u64> {
        self.runs.purge(keep).await
    }

    /// Deliver one synthetic event to verify credentials and connectivity.
    pub async fn test(&self, key: &str) -> Result<Value> {
        let s = self.sinks.get(key).await?;
        let now = Utc::now();
        let sample = ExportedEvent {
            id: Uuid::now_v7(),
            event_time: now,
            received_at: now,
            identity_id: Uuid::nil(),
            external_id: "zanshin-test".into(),
            name: "$export_test".into(),
            value: String::new(),
            metadata: json!({ "sink": key }),
            session_id: None,
            experiments: json!({}),
        };
        let started = std::time::Instant::now();
        let object = self.deliver(&s, &[sample], true).await?;
        Ok(json!({ "ok": true, "object": object, "took_ms": started.elapsed().as_millis() as u64 }))
    }

    /// Run every due sink once; called from housekeeping.
    pub async fn run_due(&self) {
        let sinks = match self.sinks.claim_due(10).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "export sinks claim failed");
                return;
            }
        };
        for s in sinks {
            if let Err(e) = self.run_sink(&s).await {
                tracing::warn!(sink = %s.key, error = %e, "export run failed");
            }
        }
    }

    pub async fn run_now(&self, key: &str) -> Result<ExportRun> {
        let s = self.sinks.get(key).await?;
        let run_id = self.run_sink(&s).await?;
        let runs = self.runs.list(s.id, 20).await?;
        runs.into_iter()
            .find(|r| r.id == run_id)
            .ok_or_else(|| Error::not_found("run"))
    }

    async fn run_sink(&self, s: &ExportSink) -> Result<Uuid> {
        let run = self.runs.start(s.id).await?;
        let after = s
            .cursor_at
            .map(|at| (at, s.cursor_id.unwrap_or(Uuid::nil())));
        let rows = match self
            .events
            .export_batch(after, &s.events, s.batch_size as i64)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.runs
                    .finish(run, "failed", 0, 0, "", &e.to_string())
                    .await?;
                self.sinks.finish(s.id, None, &e.to_string()).await?;
                return Err(e);
            }
        };
        if rows.is_empty() {
            self.runs.finish(run, "empty", 0, 0, "", "").await?;
            self.sinks.finish(s.id, None, "").await?;
            return Ok(run);
        }
        let last = rows.last().map(|r| (r.received_at, r.id));
        match self.deliver(s, &rows, false).await {
            Ok((object, bytes)) => {
                self.runs
                    .finish(run, "ok", rows.len() as i64, bytes, &object, "")
                    .await?;
                self.sinks.finish(s.id, last, "").await?;
                metrics::counter!("zanshin_export_rows_total", "sink" => s.key.clone())
                    .increment(rows.len() as u64);
                if rows.len() as i32 >= s.batch_size {
                    self.sinks.trigger(&s.key).await.ok();
                }
            }
            Err(e) => {
                self.runs
                    .finish(run, "failed", 0, 0, "", &e.to_string())
                    .await?;
                self.sinks.finish(s.id, None, &e.to_string()).await?;
                metrics::counter!("zanshin_export_failures_total", "sink" => s.key.clone())
                    .increment(1);
            }
        }
        Ok(run)
    }

    fn encode(s: &ExportSink, rows: &[ExportedEvent]) -> Result<(Vec<u8>, &'static str)> {
        let mut raw: Vec<u8> = Vec::with_capacity(rows.len() * 256);
        let content_type = match s.format {
            ExportFormat::Jsonl => {
                for r in rows {
                    serde_json::to_writer(&mut raw, r)
                        .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
                    raw.push(b'\n');
                }
                "application/x-ndjson"
            }
            ExportFormat::Csv => {
                raw.extend_from_slice(b"id,event_time,received_at,identity_id,external_id,name,value,metadata,session_id,experiments\n");
                let esc = |v: &str| -> String {
                    if v.contains([',', '"', '\n']) {
                        format!("\"{}\"", v.replace('"', "\"\""))
                    } else {
                        v.to_string()
                    }
                };
                for r in rows {
                    let line = format!(
                        "{},{},{},{},{},{},{},{},{},{}\n",
                        r.id,
                        r.event_time.to_rfc3339(),
                        r.received_at.to_rfc3339(),
                        r.identity_id,
                        esc(&r.external_id),
                        esc(&r.name),
                        esc(&r.value),
                        esc(&r.metadata.to_string()),
                        r.session_id.map(|s| s.to_string()).unwrap_or_default(),
                        esc(&r.experiments.to_string()),
                    );
                    raw.extend_from_slice(line.as_bytes());
                }
                "text/csv"
            }
        };
        if s.gzip && s.kind != SinkKind::BigQuery {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&raw)
                .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
            return Ok((
                enc.finish()
                    .map_err(|e| Error::internal(anyhow::anyhow!(e)))?,
                content_type,
            ));
        }
        Ok((raw, content_type))
    }

    fn object_name(s: &ExportSink, rows: &[ExportedEvent], test: bool) -> String {
        let prefix = s
            .config
            .get("prefix")
            .and_then(|p| p.as_str())
            .unwrap_or("zanshin/events")
            .trim_matches('/')
            .to_string();
        let first = rows.first().map(|r| r.received_at).unwrap_or_else(Utc::now);
        let ext = format!("{}{}", s.format.as_str(), if s.gzip { ".gz" } else { "" });
        let stem = if test {
            "test".to_string()
        } else {
            format!(
                "{}-{}",
                first.format("%Y%m%dT%H%M%S"),
                Uuid::now_v7().simple()
            )
        };
        format!("{prefix}/{}/{stem}.{ext}", first.format("%Y/%m/%d"))
    }

    /// Returns (object name or endpoint, bytes sent).
    async fn deliver(
        &self,
        s: &ExportSink,
        rows: &[ExportedEvent],
        test: bool,
    ) -> Result<(String, i64)> {
        match s.kind {
            SinkKind::S3 => {
                let (body, ct) = Self::encode(s, rows)?;
                let object = Self::object_name(s, rows, test);
                self.put_s3(&s.config, &object, &body, ct).await?;
                Ok((object, body.len() as i64))
            }
            SinkKind::Http => {
                let (body, ct) = Self::encode(s, rows)?;
                let url = s.config["url"].as_str().unwrap_or_default().to_string();
                let ts = Utc::now().timestamp().to_string();
                let mut req = self
                    .http
                    .post(&url)
                    .header("content-type", ct)
                    .header("x-zanshin-sink", &s.key)
                    .header("x-zanshin-rows", rows.len().to_string())
                    .header("x-zanshin-timestamp", &ts);
                if s.gzip {
                    req = req.header("content-encoding", "gzip");
                }
                if let Some(secret) = s.config["secret"].as_str().filter(|x| !x.is_empty()) {
                    req = req.header(
                        "x-zanshin-signature",
                        super::integrations::sign(secret, &ts, &body),
                    );
                }
                if let Some(h) = s.config["headers"].as_object() {
                    for (k, v) in h {
                        if let Some(v) = v.as_str() {
                            req = req.header(k.as_str(), v);
                        }
                    }
                }
                let res = req
                    .body(body.clone())
                    .send()
                    .await
                    .map_err(|e| Error::invalid(format!("http sink: {e}")))?;
                if !res.status().is_success() {
                    return Err(Error::invalid(format!("http sink status {}", res.status())));
                }
                Ok((url, body.len() as i64))
            }
            SinkKind::BigQuery => {
                let project = s.config["project"].as_str().unwrap_or_default();
                let dataset = s.config["dataset"].as_str().unwrap_or_default();
                let table = s.config["table"].as_str().unwrap_or_default();
                let token = self
                    .google_token(
                        &s.config["service_account"],
                        "https://www.googleapis.com/auth/bigquery.insertdata",
                    )
                    .await?;
                let payload = json!({
                    "skipInvalidRows": false,
                    "ignoreUnknownValues": true,
                    "rows": rows.iter().map(|r| json!({
                        "insertId": r.id,
                        "json": {
                            "id": r.id, "event_time": r.event_time.to_rfc3339(), "received_at": r.received_at.to_rfc3339(),
                            "identity_id": r.identity_id, "external_id": r.external_id, "name": r.name, "value": r.value,
                            "metadata": r.metadata.to_string(), "session_id": r.session_id, "experiments": r.experiments.to_string(),
                        }
                    })).collect::<Vec<_>>(),
                });
                let url = format!("https://bigquery.googleapis.com/bigquery/v2/projects/{project}/datasets/{dataset}/tables/{table}/insertAll");
                let body = serde_json::to_vec(&payload)
                    .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
                let res = self
                    .http
                    .post(&url)
                    .bearer_auth(token)
                    .header("content-type", "application/json")
                    .body(body.clone())
                    .send()
                    .await
                    .map_err(|e| Error::invalid(format!("bigquery: {e}")))?;
                let status = res.status();
                let v: Value = res.json().await.unwrap_or(Value::Null);
                if !status.is_success() {
                    return Err(Error::invalid(format!("bigquery status {status}: {v}")));
                }
                if let Some(errs) = v
                    .get("insertErrors")
                    .and_then(|e| e.as_array())
                    .filter(|e| !e.is_empty())
                {
                    return Err(Error::invalid(format!(
                        "bigquery rejected {} rows: {}",
                        errs.len(),
                        errs[0]
                    )));
                }
                Ok((format!("{project}.{dataset}.{table}"), body.len() as i64))
            }
        }
    }

    async fn put_s3(
        &self,
        cfg: &Value,
        object: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<()> {
        let bucket = cfg["bucket"].as_str().unwrap_or_default();
        let region = cfg["region"].as_str().unwrap_or("us-east-1");
        let access = cfg["access_key"].as_str().unwrap_or_default();
        let secret = cfg["secret_key"].as_str().unwrap_or_default();
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|e| !e.is_empty())
            .map(|e| e.trim_end_matches('/').to_string());
        let (host, path) = match &endpoint {
            Some(e) => {
                let host = e
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_string();
                (host, format!("/{bucket}/{object}"))
            }
            None => (
                format!("{bucket}.s3.{region}.amazonaws.com"),
                format!("/{object}"),
            ),
        };
        let scheme = if endpoint
            .as_deref()
            .is_some_and(|e| e.starts_with("http://"))
        {
            "http"
        } else {
            "https"
        };
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let payload_hash = hex(&Sha256::digest(body));
        let canonical_uri = uri_encode(&path, true);
        let canonical_headers = format!("content-type:{content_type}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
        let signed = "content-type;host;x-amz-content-sha256;x-amz-date";
        let canonical =
            format!("PUT\n{canonical_uri}\n\n{canonical_headers}\n{signed}\n{payload_hash}");
        let scope = format!("{date}/{region}/s3/aws4_request");
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
        let k = hmac(&k, region.as_bytes());
        let k = hmac(&k, b"s3");
        let k = hmac(&k, b"aws4_request");
        let sig = hex(&hmac(&k, to_sign.as_bytes()));
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={sig}"
        );
        let res = self
            .http
            .put(format!("{scheme}://{host}{path}"))
            .header("authorization", auth)
            .header("content-type", content_type)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| Error::invalid(format!("s3: {e}")))?;
        if !res.status().is_success() {
            let st = res.status();
            let txt = res.text().await.unwrap_or_default();
            return Err(Error::invalid(format!(
                "s3 status {st}: {}",
                txt.chars().take(300).collect::<String>()
            )));
        }
        Ok(())
    }

    async fn google_token(&self, sa: &Value, scope: &str) -> Result<String> {
        #[derive(serde::Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            scope: &'a str,
            aud: &'a str,
            iat: i64,
            exp: i64,
        }
        let email = sa["client_email"].as_str().unwrap_or_default();
        let pem = sa["private_key"].as_str().unwrap_or_default();
        let now = Utc::now().timestamp();
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| Error::invalid(format!("service account key: {e}")))?;
        let assertion = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &Claims {
                iss: email,
                scope,
                aud: "https://oauth2.googleapis.com/token",
                iat: now,
                exp: now + 3600,
            },
            &key,
        )
        .map_err(|e| Error::invalid(format!("service account jwt: {e}")))?;
        #[derive(Deserialize)]
        struct Tok {
            access_token: String,
        }
        let res = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|e| Error::invalid(format!("google oauth: {e}")))?;
        if !res.status().is_success() {
            return Err(Error::invalid(format!(
                "google oauth status {}",
                res.status()
            )));
        }
        let t: Tok = res
            .json()
            .await
            .map_err(|e| Error::invalid(format!("google oauth: {e}")))?;
        Ok(t.access_token)
    }
}
