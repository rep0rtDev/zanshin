//! Daily currency → USD rates used to normalise revenue (`metadata.amount` + `metadata.currency`).

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use zanshin_core::Result;

use crate::map_sqlx;

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct CurrencyRate {
    pub day: NaiveDate,
    pub code: String,
    pub usd_per_unit: f64,
    pub source: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct CurrencyRepo {
    pg: PgPool,
}

impl CurrencyRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    /// Rates since `since`, plus the latest earlier rate of every currency so stale currencies
    /// still convert.
    pub async fn list_since(&self, since: NaiveDate) -> Result<Vec<CurrencyRate>> {
        sqlx::query_as(
            "SELECT day, code, usd_per_unit, source, updated_at FROM currency_rates
             WHERE day >= $1
                OR (day, code) IN (SELECT max(day), code FROM currency_rates WHERE day < $1 GROUP BY code)
             ORDER BY day, code",
        )
        .bind(since)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn upsert_many(
        &self,
        day: NaiveDate,
        rates: &[(String, f64)],
        source: &str,
    ) -> Result<usize> {
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        for (code, rate) in rates {
            sqlx::query(
                "INSERT INTO currency_rates (day, code, usd_per_unit, source)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (day, code) DO UPDATE SET usd_per_unit = $3, source = $4, updated_at = now()",
            )
            .bind(day)
            .bind(code.to_ascii_uppercase())
            .bind(rate)
            .bind(source)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx)?;
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(rates.len())
    }
}
