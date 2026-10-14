//! Taxonomy service: event definitions + validation, rejected-event debugger, labels,
//! value schemas, read-only properties, geo-IP enrichment and instance settings.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zanshin_core::events::EventInput;
use zanshin_core::{Error, PropertyMap, Result};
use zanshin_store::cache::Cache;
use zanshin_store::taxonomy::{EventDefinition, TaxonomyRepo, ValueSchema};

use crate::config::TaxonomyConfig;

const DEF_TTL: Duration = Duration::from_secs(15);
pub const SETTING_TAXONOMY_MODE: &str = "taxonomy.mode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaxonomyMode {
    Off,
    Warn,
    Strict,
}

impl TaxonomyMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "warn" => Some(Self::Warn),
            "strict" => Some(Self::Strict),
            _ => None,
        }
    }
}

/// An event that failed taxonomy validation, kept for the debugger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedEvent {
    pub at: DateTime<Utc>,
    pub identity_id: Option<uuid::Uuid>,
    pub external_id: Option<String>,
    pub name: String,
    pub value: Option<String>,
    pub metadata: Option<Value>,
    pub reason: String,
    /// `true` when the event was dropped (strict mode); `false` when stored with a warning.
    pub rejected: bool,
}

struct Cached {
    at: Instant,
    defs: Arc<HashMap<String, EventDefinition>>,
    read_only: Arc<HashSet<String>>,
    schemas: Arc<HashMap<uuid::Uuid, Arc<jsonschema::Validator>>>,
    mode: TaxonomyMode,
}

#[derive(Clone)]
pub struct TaxonomyService {
    pub repo: TaxonomyRepo,
    cache: Cache,
    cfg: Arc<TaxonomyConfig>,
    snapshot: Arc<RwLock<Option<Arc<Cached>>>>,
    geo: Option<Arc<maxminddb::Reader<Vec<u8>>>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GeoInfo {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
}

impl TaxonomyService {
    pub fn new(repo: TaxonomyRepo, cache: Cache, cfg: TaxonomyConfig) -> Self {
        let geo = if cfg.geoip_db.is_empty() {
            None
        } else {
            match maxminddb::Reader::open_readfile(&cfg.geoip_db) {
                Ok(r) => {
                    tracing::info!(path = %cfg.geoip_db, "geo-ip database loaded");
                    Some(Arc::new(r))
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %cfg.geoip_db, "geo-ip database not loaded");
                    None
                }
            }
        };
        Self {
            repo,
            cache,
            cfg: Arc::new(cfg),
            snapshot: Arc::new(RwLock::new(None)),
            geo,
        }
    }

    pub fn config(&self) -> &TaxonomyConfig {
        &self.cfg
    }

    pub fn geo_enabled(&self) -> bool {
        self.geo.is_some()
    }

    /// Drop the in-memory snapshot so the next call reloads definitions.
    pub fn invalidate(&self) {
        *self.snapshot.write().unwrap() = None;
    }

    async fn snapshot(&self) -> Result<Arc<Cached>> {
        if let Some(c) = self.snapshot.read().unwrap().as_ref() {
            if c.at.elapsed() < DEF_TTL {
                return Ok(c.clone());
            }
        }
        let (defs, ro, schemas, mode) = tokio::try_join!(
            self.repo.list_event_definitions(),
            self.repo.read_only_keys(),
            self.repo.list_schemas(),
            self.repo.get_setting(SETTING_TAXONOMY_MODE),
        )?;
        let mode = mode
            .and_then(|v| v.as_str().and_then(TaxonomyMode::parse))
            .or_else(|| TaxonomyMode::parse(&self.cfg.mode))
            .unwrap_or(TaxonomyMode::Warn);
        let mut compiled = HashMap::new();
        for s in schemas {
            if let Ok(v) = jsonschema::validator_for(&s.schema) {
                compiled.insert(s.id, Arc::new(v));
            }
        }
        let c = Arc::new(Cached {
            at: Instant::now(),
            defs: Arc::new(defs.into_iter().map(|d| (d.name.clone(), d)).collect()),
            read_only: Arc::new(ro.into_iter().collect()),
            schemas: Arc::new(compiled),
            mode,
        });
        *self.snapshot.write().unwrap() = Some(c.clone());
        Ok(c)
    }

    pub async fn mode(&self) -> TaxonomyMode {
        self.snapshot()
            .await
            .map(|c| c.mode)
            .unwrap_or(TaxonomyMode::Warn)
    }

    pub async fn set_mode(&self, mode: TaxonomyMode) -> Result<()> {
        self.repo
            .set_setting(SETTING_TAXONOMY_MODE, &serde_json::to_value(mode).unwrap())
            .await?;
        self.invalidate();
        Ok(())
    }

    pub async fn definitions(&self) -> Result<Arc<HashMap<String, EventDefinition>>> {
        Ok(self.snapshot().await?.defs.clone())
    }

    /// Validate client events against the taxonomy. Returns the events to store; violations are
    /// recorded for the debugger and, in strict mode, dropped (the first error is returned when
    /// *all* events were rejected so the caller gets a useful 400).
    pub async fn check_events(
        &self,
        identity_id: uuid::Uuid,
        external_id: &str,
        inputs: Vec<EventInput>,
    ) -> Result<Vec<EventInput>> {
        let snap = match self.snapshot().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "taxonomy unavailable, accepting events unchecked");
                return Ok(inputs);
            }
        };
        if snap.mode == TaxonomyMode::Off {
            return Ok(inputs);
        }
        let mut keep = Vec::with_capacity(inputs.len());
        let mut first_err: Option<String> = None;
        let mut discovered = Vec::new();
        for input in inputs {
            if input.name.starts_with('$') {
                keep.push(input);
                continue;
            }
            let verdict = match snap.defs.get(&input.name) {
                Some(def) => validate_against(def, &input, &snap.schemas),
                None if self.cfg.auto_discover => {
                    discovered.push(input.name.clone());
                    Ok(())
                }
                None => Err(format!("event {:?} is not defined", input.name)),
            };
            match verdict {
                Ok(()) => keep.push(input),
                Err(reason) => {
                    let rejected = snap.mode == TaxonomyMode::Strict;
                    self.record_rejected(RejectedEvent {
                        at: Utc::now(),
                        identity_id: Some(identity_id),
                        external_id: Some(external_id.to_string()),
                        name: input.name.clone(),
                        value: input.value.clone(),
                        metadata: input.metadata.clone().map(Value::Object),
                        reason: reason.clone(),
                        rejected,
                    })
                    .await;
                    metrics::counter!("zanshin_events_taxonomy_violations_total", "dropped" => if rejected { "true" } else { "false" }).increment(1);
                    if rejected {
                        first_err.get_or_insert(reason);
                    } else {
                        keep.push(input);
                    }
                }
            }
        }
        if !discovered.is_empty() {
            let repo = self.repo.clone();
            let me = self.clone();
            tokio::spawn(async move {
                let mut any = false;
                for n in discovered {
                    if repo.discover_event(&n).await.unwrap_or(false) {
                        any = true;
                    }
                }
                if any {
                    me.invalidate();
                }
            });
        }
        if keep.is_empty() {
            if let Some(e) = first_err {
                return Err(Error::invalid(e));
            }
        }
        Ok(keep)
    }

    async fn record_rejected(&self, r: RejectedEvent) {
        let key = self.cache.key(&["taxonomy", "rejected"]);
        if let Err(e) = self
            .cache
            .push_capped(&key, &r, self.cfg.rejected_keep)
            .await
        {
            tracing::debug!(error = %e, "rejected event not recorded");
        }
    }

    pub async fn rejected(&self, limit: usize) -> Result<Vec<RejectedEvent>> {
        let key = self.cache.key(&["taxonomy", "rejected"]);
        self.cache
            .list_json(&key, limit.min(self.cfg.rejected_keep))
            .await
    }

    pub async fn clear_rejected(&self) -> Result<()> {
        let key = self.cache.key(&["taxonomy", "rejected"]);
        self.cache.del(&key).await
    }

    /// Reject client writes to custom properties marked read-only.
    pub async fn check_client_custom_write(&self, patch: &PropertyMap) -> Result<()> {
        let snap = self.snapshot().await?;
        if let Some(k) = patch.keys().find(|k| snap.read_only.contains(*k)) {
            return Err(Error::invalid(format!(
                "property {k:?} is read-only and can only be set from the server"
            )));
        }
        Ok(())
    }

    /// Validate a value against a stored schema (flag/live-event values).
    pub async fn validate_value(&self, schema_id: uuid::Uuid, value: &Value) -> Result<()> {
        let snap = self.snapshot().await?;
        let Some(v) = snap.schemas.get(&schema_id) else {
            return Err(Error::invalid("schema not found or does not compile"));
        };
        validate_with(v, value).map_err(Error::invalid)
    }

    pub async fn schema_by_id(&self, id: uuid::Uuid) -> Result<Option<ValueSchema>> {
        self.repo.get_schema(id).await
    }

    /// Compile and test a schema against sample values without storing it.
    pub fn dry_run_schema(schema: &Value, samples: &[Value]) -> Result<Vec<Option<String>>> {
        let v = jsonschema::validator_for(schema)
            .map_err(|e| Error::invalid(format!("invalid JSON Schema: {e}")))?;
        Ok(samples.iter().map(|s| validate_with(&v, s).err()).collect())
    }

    /// Fill default properties the client did not send from request headers and geo-IP.
    /// Header names: `X-Zanshin-Platform`, `X-Zanshin-OS`, `X-Zanshin-OS-Version`,
    /// `X-Zanshin-Device`, `X-Zanshin-App-Version`, `X-Zanshin-Timezone`, `Accept-Language`;
    /// country falls back to `CF-IPCountry` / `X-Country` when no geo database is configured.
    pub fn enrich_defaults(
        &self,
        headers: &http::HeaderMap,
        ip: Option<IpAddr>,
        default: &mut PropertyMap,
    ) {
        let hdr = |n: &str| {
            headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty() && v.len() <= 128)
        };
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                default.entry(k.to_string()).or_insert(Value::String(v));
            }
        };
        put("platform", hdr("x-zanshin-platform"));
        put("os", hdr("x-zanshin-os"));
        put("os_version", hdr("x-zanshin-os-version"));
        put("device", hdr("x-zanshin-device"));
        put("app_version", hdr("x-zanshin-app-version"));
        put("timezone", hdr("x-zanshin-timezone"));
        if let Some(al) = hdr("accept-language") {
            let locale = al
                .split(',')
                .next()
                .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
                .filter(|s| !s.is_empty() && s != "*");
            if let Some(l) = locale {
                let lang = l.split(['-', '_']).next().unwrap_or(&l).to_lowercase();
                put("locale", Some(l.clone()));
                put("language", Some(lang));
            }
        }
        let geo = ip.and_then(|ip| self.lookup_geo(ip));
        match geo {
            Some(g) => {
                put("country", g.country);
                put("region", g.region);
                put("city", g.city);
                put("timezone", g.timezone);
            }
            None => put(
                "country",
                hdr("cf-ipcountry")
                    .or_else(|| hdr("x-country"))
                    .map(|c| c.to_uppercase())
                    .filter(|c| c.len() == 2),
            ),
        }
    }

    pub fn lookup_geo(&self, ip: IpAddr) -> Option<GeoInfo> {
        let reader = self.geo.as_ref()?;
        let city: maxminddb::geoip2::City = reader.lookup(ip).ok()?.decode().ok().flatten()?;
        Some(GeoInfo {
            country: city.country.iso_code.map(str::to_string),
            region: city
                .subdivisions
                .into_iter()
                .next()
                .and_then(|s| s.iso_code.map(str::to_string)),
            city: city.city.names.english.map(str::to_string),
            timezone: city.location.time_zone.map(str::to_string),
        })
    }
}

fn validate_against(
    def: &EventDefinition,
    input: &EventInput,
    schemas: &HashMap<uuid::Uuid, Arc<jsonschema::Validator>>,
) -> std::result::Result<(), String> {
    let _ = schemas;
    let value = input.value.as_deref().unwrap_or("");
    match def.value_type.as_str() {
        "number" => {
            if value.is_empty() || value.parse::<f64>().is_err() {
                return Err(format!("event {:?} requires a numeric value", def.name));
            }
        }
        "string" => {
            if value.is_empty() {
                return Err(format!("event {:?} requires a string value", def.name));
            }
        }
        "object" => {
            if serde_json::from_str::<Map<String, Value>>(value).is_err() {
                return Err(format!("event {:?} requires a JSON object value", def.name));
            }
        }
        "revenue" => {
            let m = input.metadata.as_ref();
            let amount = m.and_then(|m| m.get("amount")).and_then(|a| a.as_f64());
            let currency = m.and_then(|m| m.get("currency")).and_then(|c| c.as_str());
            if amount.is_none() || currency.is_none_or(|c| c.len() != 3) {
                return Err(format!(
                    "event {:?} requires metadata.amount (number) and metadata.currency (ISO 4217)",
                    def.name
                ));
            }
        }
        _ => {}
    }
    if let Some(schema) = def.metadata_schema.as_ref().filter(|s| s.is_object()) {
        let meta = input
            .metadata
            .clone()
            .map(Value::Object)
            .unwrap_or(Value::Object(Map::new()));
        let v = jsonschema::validator_for(schema)
            .map_err(|e| format!("metadata schema of {:?} does not compile: {e}", def.name))?;
        validate_with(&v, &meta).map_err(|e| format!("event {:?} metadata: {e}", def.name))?;
    }
    Ok(())
}

fn validate_with(v: &jsonschema::Validator, value: &Value) -> std::result::Result<(), String> {
    match v.iter_errors(value).next() {
        None => Ok(()),
        Some(e) => Err(format!("{} at {}", e, e.instance_path)),
    }
}
