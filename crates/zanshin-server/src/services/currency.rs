//! Currency normalisation: in-memory table of daily USD rates loaded from Postgres, optionally
//! refreshed from a remote feed (`taxonomy.currency_rates_url`, USD base, frankfurter-style JSON).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use zanshin_core::{Error, Result};
use zanshin_store::currency::{CurrencyRate, CurrencyRepo};

type RateTable = HashMap<String, Vec<(NaiveDate, f64)>>;

const KEEP_DAYS: i64 = 800;
const REMOTE_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone)]
pub struct CurrencyService {
    repo: CurrencyRepo,
    url: Option<String>,
    rates: Arc<RwLock<RateTable>>,
    last_remote: Arc<Mutex<Option<Instant>>>,
}

#[derive(Deserialize)]
struct Feed {
    #[serde(default)]
    date: Option<NaiveDate>,
    rates: HashMap<String, f64>,
}

impl CurrencyService {
    pub fn new(repo: CurrencyRepo, url: Option<String>) -> Self {
        Self {
            repo,
            url: url.filter(|u| !u.trim().is_empty()),
            rates: Arc::default(),
            last_remote: Arc::default(),
        }
    }

    /// (Re)load the table from Postgres. Called at startup and after every write.
    pub async fn load(&self) -> Result<()> {
        let since = Utc::now().date_naive() - chrono::Duration::days(KEEP_DAYS);
        let rows = self.repo.list_since(since).await?;
        let mut map: RateTable = HashMap::new();
        for r in rows {
            map.entry(r.code).or_default().push((r.day, r.usd_per_unit));
        }
        if let Ok(mut g) = self.rates.write() {
            *g = map;
        }
        Ok(())
    }

    /// USD value of `amount` in `code` on `day`: the latest rate at or before `day`, otherwise the
    /// earliest known one. `None` when the currency is unknown.
    pub fn to_usd(&self, amount: f64, code: &str, day: NaiveDate) -> Option<f64> {
        let code = code.trim().to_ascii_uppercase();
        if code == "USD" {
            return Some(amount);
        }
        let g = self.rates.read().ok()?;
        let series = g.get(&code)?;
        let rate = series
            .iter()
            .rev()
            .find(|(d, _)| *d <= day)
            .or_else(|| series.first())
            .map(|(_, r)| *r)?;
        Some((amount * rate * 1_000_000.0).round() / 1_000_000.0)
    }

    pub fn known_currencies(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .rates
            .read()
            .map(|g| g.keys().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    pub async fn list_since(&self, since: NaiveDate) -> Result<Vec<CurrencyRate>> {
        self.repo.list_since(since).await
    }

    pub async fn upsert(
        &self,
        day: NaiveDate,
        rates: &[(String, f64)],
        source: &str,
    ) -> Result<usize> {
        for (code, rate) in rates {
            if code.trim().len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(Error::invalid(format!("currency code {code:?} must be 3 letters")));
            }
            if !(rate.is_finite() && *rate > 0.0) {
                return Err(Error::invalid(format!("rate for {code} must be > 0")));
            }
        }
        let n = self.repo.upsert_many(day, rates, source).await?;
        self.load().await?;
        Ok(n)
    }

    pub fn remote_configured(&self) -> bool {
        self.url.is_some()
    }

    /// Fetch today's rates from the configured feed (`{"date": "...", "rates": {"EUR": 0.92}}`,
    /// units per USD). Throttled to once per 6h unless `force`. Returns rows written.
    pub async fn refresh_remote(&self, force: bool) -> Result<usize> {
        let Some(url) = &self.url else {
            return Ok(0);
        };
        if !force {
            let g = self.last_remote.lock().ok();
            if g.as_ref()
                .and_then(|g| **g)
                .is_some_and(|t| t.elapsed() < REMOTE_EVERY)
            {
                return Ok(0);
            }
        }
        if let Ok(mut g) = self.last_remote.lock() {
            *g = Some(Instant::now());
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
        let feed: Feed = client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::unavailable(format!("currency feed: {e}")))?
            .json()
            .await
            .map_err(|e| Error::unavailable(format!("currency feed: {e}")))?;
        let day = feed.date.unwrap_or_else(|| Utc::now().date_naive());
        let rates: Vec<(String, f64)> = feed
            .rates
            .into_iter()
            .filter(|(_, per_usd)| per_usd.is_finite() && *per_usd > 0.0)
            .map(|(code, per_usd)| (code, 1.0 / per_usd))
            .collect();
        self.upsert(day, &rates, "remote").await
    }
}
