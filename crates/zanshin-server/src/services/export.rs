//! Event export sinks: periodic delivery of new events to S3, an HTTP endpoint, or BigQuery.

use std::{io::Write, sync::Arc};

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
    notifier: std::sync::Arc<std::sync::RwLock<Option<super::notifications::NotificationService>>>,
}

pub fn mask_config(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    if zanshin_core::integrations::is_secret_key(k) && v.is_string() {
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
    /// Route failed runs to notification channels.
    pub fn attach_notifier(&self, n: super::notifications::NotificationService) {
        *self.notifier.write().unwrap() = Some(n);
    }

    pub fn new(sinks: ExportSinkRepo, runs: ExportRunRepo, events: EventService) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            notifier: Default::default(),
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
                if let Some(n) = self.notifier.read().unwrap().clone() {
                    n.notify(
                        zanshin_core::notifications::Notice::new(
                            "export.failed",
                            format!("Export sink `{}` failed", s.key),
                            e.to_string(),
                        )
                        .field("sink", s.key.clone())
                        .field("kind", s.kind.as_str()),
                    );
                }
            }
        }
        Ok(run)
    }

    /// `$event_error` rows carry the validation reason as a top-level `_error` column so
    /// warehouse users can filter rejected events without parsing metadata.
    fn error_of(r: &ExportedEvent) -> Option<String> {
        (r.name == zanshin_core::events::EV_EVENT_ERROR).then(|| {
            r.metadata
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("rejected")
                .to_string()
        })
    }

    fn encode(s: &ExportSink, rows: &[ExportedEvent]) -> Result<(Vec<u8>, &'static str)> {
        if s.format == ExportFormat::Parquet {
            return Ok((
                Self::encode_parquet(rows)?,
                "application/vnd.apache.parquet",
            ));
        }
        let mut raw: Vec<u8> = Vec::with_capacity(rows.len() * 256);
        let content_type = match s.format {
            ExportFormat::Jsonl => {
                for r in rows {
                    let mut v =
                        serde_json::to_value(r).map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
                    if let (Some(err), Some(obj)) = (Self::error_of(r), v.as_object_mut()) {
                        obj.insert("_error".into(), Value::String(err));
                    }
                    serde_json::to_writer(&mut raw, &v)
                        .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
                    raw.push(b'\n');
                }
                "application/x-ndjson"
            }
            ExportFormat::Parquet => unreachable!(),
            ExportFormat::Csv => {
                raw.extend_from_slice(b"id,event_time,received_at,identity_id,external_id,name,value,metadata,session_id,experiments,_error\n");
                let esc = |v: &str| -> String {
                    if v.contains([',', '"', '\n']) {
                        format!("\"{}\"", v.replace('"', "\"\""))
                    } else {
                        v.to_string()
                    }
                };
                for r in rows {
                    let line = format!(
                        "{},{},{},{},{},{},{},{},{},{},{}\n",
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
                        esc(&Self::error_of(r).unwrap_or_default()),
                    );
                    raw.extend_from_slice(line.as_bytes());
                }
                "text/csv"
            }
        };
        if s.gzip && s.kind != SinkKind::BigQuery && s.format != ExportFormat::Parquet {
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

    fn encode_parquet(rows: &[ExportedEvent]) -> Result<Vec<u8>> {
        use parquet::{
            basic::{Compression, ConvertedType, Repetition, Type as PhysicalType},
            column::writer::ColumnWriter,
            data_type::ByteArray,
            file::{properties::WriterProperties, writer::SerializedFileWriter},
            schema::types::Type,
        };
        let perr = |e: parquet::errors::ParquetError| Error::internal(anyhow::anyhow!(e));
        let str_col = |name: &str, required: bool| -> Result<Arc<Type>> {
            Type::primitive_type_builder(name, PhysicalType::BYTE_ARRAY)
                .with_converted_type(ConvertedType::UTF8)
                .with_repetition(if required {
                    Repetition::REQUIRED
                } else {
                    Repetition::OPTIONAL
                })
                .build()
                .map(Arc::new)
                .map_err(perr)
        };
        let ts_col = |name: &str| -> Result<Arc<Type>> {
            Type::primitive_type_builder(name, PhysicalType::INT64)
                .with_converted_type(ConvertedType::TIMESTAMP_MILLIS)
                .with_repetition(Repetition::REQUIRED)
                .build()
                .map(Arc::new)
                .map_err(perr)
        };
        let schema = Arc::new(
            Type::group_type_builder("event")
                .with_fields(vec![
                    str_col("id", true)?,
                    ts_col("event_time")?,
                    ts_col("received_at")?,
                    str_col("identity_id", true)?,
                    str_col("external_id", true)?,
                    str_col("name", true)?,
                    str_col("value", true)?,
                    str_col("metadata", true)?,
                    str_col("session_id", false)?,
                    str_col("experiments", true)?,
                    str_col("_error", false)?,
                ])
                .build()
                .map_err(perr)?,
        );
        enum Col {
            Str(Vec<Option<String>>),
            Ts(Vec<i64>),
        }
        let cols = [
            Col::Str(rows.iter().map(|r| Some(r.id.to_string())).collect()),
            Col::Ts(
                rows.iter()
                    .map(|r| r.event_time.timestamp_millis())
                    .collect(),
            ),
            Col::Ts(
                rows.iter()
                    .map(|r| r.received_at.timestamp_millis())
                    .collect(),
            ),
            Col::Str(
                rows.iter()
                    .map(|r| Some(r.identity_id.to_string()))
                    .collect(),
            ),
            Col::Str(rows.iter().map(|r| Some(r.external_id.clone())).collect()),
            Col::Str(rows.iter().map(|r| Some(r.name.clone())).collect()),
            Col::Str(rows.iter().map(|r| Some(r.value.clone())).collect()),
            Col::Str(rows.iter().map(|r| Some(r.metadata.to_string())).collect()),
            Col::Str(
                rows.iter()
                    .map(|r| r.session_id.map(|s| s.to_string()))
                    .collect(),
            ),
            Col::Str(
                rows.iter()
                    .map(|r| Some(r.experiments.to_string()))
                    .collect(),
            ),
            Col::Str(rows.iter().map(Self::error_of).collect()),
        ];
        let props = Arc::new(
            WriterProperties::builder()
                .set_compression(Compression::SNAPPY)
                .set_created_by("zanshin".into())
                .build(),
        );
        let mut out: Vec<u8> = Vec::with_capacity(rows.len() * 128);
        let mut writer = SerializedFileWriter::new(&mut out, schema, props).map_err(perr)?;
        let mut rg = writer.next_row_group().map_err(perr)?;
        for col in cols.iter() {
            let mut cw = rg.next_column().map_err(perr)?.ok_or_else(|| {
                Error::internal(anyhow::anyhow!("parquet schema/column mismatch"))
            })?;
            match (cw.untyped(), col) {
                (ColumnWriter::ByteArrayColumnWriter(w), Col::Str(vals)) => {
                    let defs: Vec<i16> = vals.iter().map(|v| i16::from(v.is_some())).collect();
                    let present: Vec<ByteArray> = vals
                        .iter()
                        .flatten()
                        .map(|v| ByteArray::from(v.as_str()))
                        .collect();
                    w.write_batch(&present, Some(&defs), None).map_err(perr)?;
                }
                (ColumnWriter::Int64ColumnWriter(w), Col::Ts(vals)) => {
                    w.write_batch(vals, None, None).map_err(perr)?;
                }
                _ => {
                    return Err(Error::internal(anyhow::anyhow!(
                        "parquet column type mismatch"
                    )))
                }
            }
            cw.close().map_err(perr)?;
        }
        rg.close().map_err(perr)?;
        writer.close().map_err(perr)?;
        Ok(out)
    }

    fn partition_by_event(s: &ExportSink) -> bool {
        s.config.get("partition").and_then(|p| p.as_str()) == Some("event")
    }

    /// Upload rows to S3 as one object, or one object per event name when `partition = event`.
    /// Returns (object names, bytes).
    async fn stage_s3(
        &self,
        s: &ExportSink,
        rows: &[ExportedEvent],
        test: bool,
    ) -> Result<(Vec<String>, i64)> {
        let mut groups: Vec<(Option<String>, Vec<&ExportedEvent>)> = Vec::new();
        if Self::partition_by_event(s) {
            let mut by: std::collections::BTreeMap<&str, Vec<&ExportedEvent>> = Default::default();
            for r in rows {
                by.entry(r.name.as_str()).or_default().push(r);
            }
            for (name, g) in by {
                groups.push((Some(name.to_string()), g));
            }
        } else {
            groups.push((None, rows.iter().collect()));
        }
        let mut objects = Vec::with_capacity(groups.len());
        let mut bytes = 0i64;
        for (part, g) in groups {
            let owned: Vec<ExportedEvent> = g.into_iter().cloned().collect();
            let (body, ct) = Self::encode(s, &owned)?;
            let object = Self::object_name(s, &owned, test, part.as_deref());
            self.put_s3(&s.config, &object, &body, ct).await?;
            bytes += body.len() as i64;
            objects.push(object);
        }
        Ok((objects, bytes))
    }

    fn object_name(
        s: &ExportSink,
        rows: &[ExportedEvent],
        test: bool,
        partition: Option<&str>,
    ) -> String {
        let prefix = s
            .config
            .get("prefix")
            .and_then(|p| p.as_str())
            .unwrap_or("zanshin/events")
            .trim_matches('/')
            .to_string();
        let first = rows.first().map(|r| r.received_at).unwrap_or_else(Utc::now);
        let gz = s.gzip && s.format != ExportFormat::Parquet;
        let ext = format!("{}{}", s.format.as_str(), if gz { ".gz" } else { "" });
        let part = partition
            .map(|p| {
                let safe: String = p
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!("event={safe}/")
            })
            .unwrap_or_default();
        let stem = if test {
            "test".to_string()
        } else {
            format!(
                "{}-{}",
                first.format("%Y%m%dT%H%M%S"),
                Uuid::now_v7().simple()
            )
        };
        format!("{prefix}/{part}{}/{stem}.{ext}", first.format("%Y/%m/%d"))
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
                let (objects, bytes) = self.stage_s3(s, rows, test).await?;
                Ok((Self::describe(&objects), bytes))
            }
            SinkKind::Snowflake => {
                let (objects, bytes) = self.stage_s3(s, rows, test).await?;
                self.snowflake_copy(s, &objects).await?;
                Ok((Self::describe(&objects), bytes))
            }
            SinkKind::Redshift => {
                let (objects, bytes) = self.stage_s3(s, rows, test).await?;
                self.redshift_copy(s, &objects).await?;
                Ok((Self::describe(&objects), bytes))
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
                            "_error": Self::error_of(r),
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

    fn describe(objects: &[String]) -> String {
        match objects {
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
            [] => String::new(),
        }
    }

    fn file_format_sql(s: &ExportSink, snowflake: bool) -> String {
        let gz = s.gzip && s.format != ExportFormat::Parquet;
        if snowflake {
            match s.format {
                ExportFormat::Jsonl => format!(
                    "FILE_FORMAT = (TYPE = JSON COMPRESSION = {}) MATCH_BY_COLUMN_NAME = CASE_INSENSITIVE",
                    if gz { "GZIP" } else { "NONE" }
                ),
                ExportFormat::Csv => format!(
                    "FILE_FORMAT = (TYPE = CSV SKIP_HEADER = 1 FIELD_OPTIONALLY_ENCLOSED_BY = '\"' COMPRESSION = {})",
                    if gz { "GZIP" } else { "NONE" }
                ),
                ExportFormat::Parquet => {
                    "FILE_FORMAT = (TYPE = PARQUET) MATCH_BY_COLUMN_NAME = CASE_INSENSITIVE".into()
                }
            }
        } else {
            match s.format {
                ExportFormat::Jsonl => format!(
                    "FORMAT AS JSON 'auto' TIMEFORMAT 'auto'{}",
                    if gz { " GZIP" } else { "" }
                ),
                ExportFormat::Csv => format!(
                    "FORMAT AS CSV IGNOREHEADER 1 TIMEFORMAT 'auto'{}",
                    if gz { " GZIP" } else { "" }
                ),
                ExportFormat::Parquet => "FORMAT AS PARQUET".into(),
            }
        }
    }

    fn ident(s: &str) -> Result<&str> {
        if s.is_empty()
            || !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$')
        {
            return Err(Error::invalid(format!("invalid SQL identifier {s:?}")));
        }
        Ok(s)
    }

    /// `COPY INTO` the staged objects through the Snowflake SQL API (key-pair JWT auth). The
    /// external stage must point at the sink's bucket + prefix.
    async fn snowflake_copy(&self, s: &ExportSink, objects: &[String]) -> Result<()> {
        let c = &s.config;
        let get = |k: &str| c[k].as_str().unwrap_or_default().to_string();
        let account = get("account").to_uppercase();
        let user = get("user").to_uppercase();
        let fp = get("public_key_fp");
        let fp = if fp.starts_with("SHA256:") {
            fp
        } else {
            format!("SHA256:{fp}")
        };
        let now = Utc::now().timestamp();
        let claims = json!({
            "iss": format!("{account}.{user}.{fp}"),
            "sub": format!("{account}.{user}"),
            "iat": now,
            "exp": now + 55 * 60,
        });
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(get("private_key").as_bytes())
            .map_err(|e| Error::invalid(format!("snowflake private_key: {e}")))?;
        let jwt = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &key,
        )
        .map_err(|e| Error::invalid(format!("snowflake jwt: {e}")))?;
        let table = format!(
            "{}.{}.{}",
            Self::ident(&get("database"))?,
            Self::ident(&get("schema"))?,
            Self::ident(&get("table"))?
        );
        let stage = Self::ident(&get("stage"))?.to_string();
        let prefix = c["prefix"]
            .as_str()
            .unwrap_or("zanshin/events")
            .trim_matches('/')
            .to_string();
        let files = objects
            .iter()
            .map(|o| {
                let rel = o.strip_prefix(&format!("{prefix}/")).unwrap_or(o);
                format!("'{}'", rel.replace('\'', ""))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "COPY INTO {table} FROM @{stage} FILES = ({files}) {} ON_ERROR = ABORT_STATEMENT",
            Self::file_format_sql(s, true)
        );
        let mut body = json!({ "statement": sql, "timeout": 600, "database": get("database"), "schema": get("schema") });
        for k in ["warehouse", "role"] {
            if let Some(v) = c[k].as_str().filter(|v| !v.is_empty()) {
                body[k] = Value::String(v.to_string());
            }
        }
        let host = get("host");
        let host = host
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        let res = self
            .http
            .post(format!("https://{host}/api/v2/statements"))
            .bearer_auth(jwt)
            .header("X-Snowflake-Authorization-Token-Type", "KEYPAIR_JWT")
            .header("accept", "application/json")
            .header("user-agent", "zanshin")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::invalid(format!("snowflake: {e}")))?;
        let st = res.status();
        if !st.is_success() {
            let txt = res.text().await.unwrap_or_default();
            return Err(Error::invalid(format!(
                "snowflake status {st}: {}",
                txt.chars().take(400).collect::<String>()
            )));
        }
        Ok(())
    }

    /// `COPY` the staged objects through the Redshift Data API (SigV4 with the sink's keys).
    async fn redshift_copy(&self, s: &ExportSink, objects: &[String]) -> Result<()> {
        let c = &s.config;
        let get = |k: &str| c[k].as_str().unwrap_or_default().to_string();
        let region = c["region"].as_str().unwrap_or("us-east-1").to_string();
        let bucket = get("bucket");
        let role = get("iam_role").replace('\'', "");
        let table = Self::ident(&get("table"))?.to_string();
        let fmt = Self::file_format_sql(s, false);
        let mut statements = Vec::with_capacity(objects.len());
        for o in objects {
            statements.push(format!(
                "COPY {table} FROM 's3://{bucket}/{}' IAM_ROLE '{role}' {fmt}",
                o.replace('\'', "")
            ));
        }
        let mut body = json!({ "Database": get("database") });
        if statements.len() == 1 {
            body["Sql"] = Value::String(statements.remove(0));
        } else {
            body["Sqls"] = Value::Array(statements.into_iter().map(Value::String).collect());
        }
        if let Some(v) = c["cluster_id"].as_str().filter(|v| !v.is_empty()) {
            body["ClusterIdentifier"] = Value::String(v.to_string());
        }
        if let Some(v) = c["workgroup"].as_str().filter(|v| !v.is_empty()) {
            body["WorkgroupName"] = Value::String(v.to_string());
        }
        if let Some(v) = c["db_user"].as_str().filter(|v| !v.is_empty()) {
            body["DbUser"] = Value::String(v.to_string());
        }
        if let Some(v) = c["secret_arn"].as_str().filter(|v| !v.is_empty()) {
            body["SecretArn"] = Value::String(v.to_string());
        }
        let payload = serde_json::to_vec(&body).map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
        let host = format!("redshift-data.{region}.amazonaws.com");
        let target = if body.get("Sqls").is_some() {
            "RedshiftData.BatchExecuteStatement"
        } else {
            "RedshiftData.ExecuteStatement"
        };
        let (auth, amz_date, _) = Self::sigv4(
            "POST",
            &host,
            "/",
            "redshift-data",
            &region,
            &get("access_key"),
            &get("secret_key"),
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", target),
            ],
            &payload,
        );
        let res = self
            .http
            .post(format!("https://{host}/"))
            .header("authorization", auth)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header("x-amz-date", amz_date)
            .body(payload)
            .send()
            .await
            .map_err(|e| Error::invalid(format!("redshift: {e}")))?;
        let st = res.status();
        if !st.is_success() {
            let txt = res.text().await.unwrap_or_default();
            return Err(Error::invalid(format!(
                "redshift status {st}: {}",
                txt.chars().take(400).collect::<String>()
            )));
        }
        Ok(())
    }

    /// AWS SigV4 for a single request. `extra` are additional headers to sign (lowercase names,
    /// sorted together with `host` and `x-amz-date`). Returns (authorization, x-amz-date, payload sha256).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sigv4(
        method: &str,
        host: &str,
        path: &str,
        service: &str,
        region: &str,
        access: &str,
        secret: &str,
        extra: &[(&str, &str)],
        payload: &[u8],
    ) -> (String, String, String) {
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let payload_hash = hex(&Sha256::digest(payload));
        let mut headers: Vec<(String, String)> = extra
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
            .collect();
        headers.push(("host".into(), host.to_string()));
        headers.push(("x-amz-date".into(), amz_date.clone()));
        headers.sort();
        let canonical_headers = headers
            .iter()
            .map(|(k, v)| format!("{k}:{v}\n"))
            .collect::<String>();
        let signed = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical = format!(
            "{method}\n{}\n\n{canonical_headers}\n{signed}\n{payload_hash}",
            uri_encode(path, true)
        );
        let scope = format!("{date}/{region}/{service}/aws4_request");
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
        let k = hmac(&k, region.as_bytes());
        let k = hmac(&k, service.as_bytes());
        let k = hmac(&k, b"aws4_request");
        let sig = hex(&hmac(&k, to_sign.as_bytes()));
        (
            format!("AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={sig}"),
            amz_date,
            payload_hash,
        )
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
        let (auth, amz_date, payload_hash) = Self::sigv4(
            "PUT",
            &host,
            &path,
            "s3",
            region,
            access,
            secret,
            &[
                ("content-type", content_type),
                ("x-amz-content-sha256", &hex(&Sha256::digest(body))),
            ],
            body,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    fn sink(format: ExportFormat, gzip: bool) -> ExportSink {
        let now = Utc::now();
        ExportSink {
            id: Uuid::nil(),
            key: "s".into(),
            name: String::new(),
            kind: SinkKind::S3,
            config: json!({"bucket": "b", "region": "r", "access_key": "a", "secret_key": "s"}),
            format,
            gzip,
            enabled: true,
            events: vec![],
            interval_secs: 300,
            batch_size: 100,
            cursor_at: None,
            cursor_id: None,
            next_run_at: None,
            last_run_at: None,
            last_error: String::new(),
            tags: vec![],
            version: 1,
            created_at: now,
            updated_at: now,
        }
    }

    fn rows() -> Vec<ExportedEvent> {
        let t = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        vec![
            ExportedEvent {
                id: Uuid::now_v7(),
                event_time: t,
                received_at: t,
                identity_id: Uuid::now_v7(),
                external_id: "p1".into(),
                name: "purchaseCompleted".into(),
                value: "sku".into(),
                metadata: json!({"amount": 1}),
                session_id: None,
                experiments: json!({}),
            },
            ExportedEvent {
                id: Uuid::now_v7(),
                event_time: t,
                received_at: t,
                identity_id: Uuid::now_v7(),
                external_id: "p2".into(),
                name: zanshin_core::events::EV_EVENT_ERROR.into(),
                value: String::new(),
                metadata: json!({"event": "bad", "reason": "unknown event"}),
                session_id: Some(Uuid::now_v7()),
                experiments: json!({}),
            },
        ]
    }

    #[test]
    fn jsonl_and_csv_carry_error_column() {
        let (body, _) = ExportService::encode(&sink(ExportFormat::Jsonl, false), &rows()).unwrap();
        let text = String::from_utf8(body).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].contains("_error"));
        assert!(lines[1].contains("\"_error\":\"unknown event\""));
        let (body, _) = ExportService::encode(&sink(ExportFormat::Csv, false), &rows()).unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.lines().next().unwrap().ends_with(",_error"));
        assert!(text.lines().nth(2).unwrap().ends_with(",unknown event"));
    }

    #[test]
    fn parquet_roundtrip() {
        let s = sink(ExportFormat::Parquet, true);
        let (body, ct) = ExportService::encode(&s, &rows()).unwrap();
        assert_eq!(ct, "application/vnd.apache.parquet");
        assert_eq!(&body[..4], b"PAR1");
        let reader = SerializedFileReader::new(bytes::Bytes::from(body)).unwrap();
        let meta = reader.metadata();
        assert_eq!(meta.file_metadata().num_rows(), 2);
        let names: Vec<String> = meta
            .file_metadata()
            .schema_descr()
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        assert_eq!(names[0], "id");
        assert_eq!(names.last().unwrap(), "_error");
        let got: Vec<String> = reader
            .get_row_iter(None)
            .unwrap()
            .map(|r| r.unwrap().to_string())
            .collect();
        assert!(got[1].contains("unknown event"), "{}", got[1]);
        assert!(got[0].contains("purchaseCompleted"));
        // object name: no .gz for parquet, event partition folder
        let name = ExportService::object_name(&s, &rows(), false, Some("purchaseCompleted"));
        assert!(
            name.starts_with("zanshin/events/event=purchaseCompleted/2026/01/02/"),
            "{name}"
        );
        assert!(name.ends_with(".parquet"));
    }

    #[test]
    fn sigv4_is_deterministic_shape() {
        let (auth, date, hash) = ExportService::sigv4(
            "POST",
            "redshift-data.us-east-1.amazonaws.com",
            "/",
            "redshift-data",
            "us-east-1",
            "AKIA",
            "secret",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", "RedshiftData.ExecuteStatement"),
            ],
            b"{}",
        );
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIA/"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date;x-amz-target,"));
        assert_eq!(date.len(), 16);
        assert_eq!(hash.len(), 64);
    }
}
