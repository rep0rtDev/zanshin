//! Redis-backed cache and pub/sub bus.

use std::time::Duration;

use anyhow::Context;
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{de::DeserializeOwned, Serialize};
use zanshin_core::{Error, Result};

use crate::RedisConfig;

const TOKEN_BUCKET: &str = r#"
local tokens = tonumber(redis.call('HGET', KEYS[1], 't'))
local ts = tonumber(redis.call('HGET', KEYS[1], 'ts'))
local cap = tonumber(ARGV[1])
local rate = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local cost = tonumber(ARGV[4])
if tokens == nil or ts == nil then
  tokens = cap
  ts = now
end
local delta = now - ts
if delta < 0 then delta = 0 end
tokens = math.min(cap, tokens + delta * rate)
local allowed = 0
local retry = 0
if tokens >= cost then
  tokens = tokens - cost
  allowed = 1
else
  retry = (cost - tokens) / rate
end
redis.call('HSET', KEYS[1], 't', tokens, 'ts', now)
redis.call('PEXPIRE', KEYS[1], math.ceil(cap / rate * 1000) + 1000)
return {allowed, math.floor(tokens), tostring(retry)}
"#;

#[derive(Clone)]
pub struct Cache {
    conn: ConnectionManager,
    prefix: String,
}

impl Cache {
    pub async fn connect(cfg: &RedisConfig) -> Result<Self> {
        let client = redis::Client::open(cfg.url.as_str()).context("parsing redis url")?;
        let conn = ConnectionManager::new(client)
            .await
            .context("connecting to redis")?;
        Ok(Self {
            conn,
            prefix: cfg.key_prefix.clone(),
        })
    }

    pub fn key(&self, parts: &[&str]) -> String {
        let mut k = self.prefix.clone();
        for p in parts {
            k.push(':');
            k.push_str(p);
        }
        k
    }

    /// Token-bucket admission check executed atomically in Redis.
    ///
    /// Returns `(allowed, remaining_tokens, retry_after_secs)`. The bucket holds `capacity`
    /// tokens and refills at `refill_per_sec`; each call takes `cost` tokens when available.
    pub async fn take_tokens(
        &self,
        key: &str,
        capacity: u32,
        refill_per_sec: f64,
        cost: u32,
    ) -> Result<(bool, u32, f64)> {
        let mut c = self.conn.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let (allowed, remaining, retry): (i64, i64, String) = redis::Script::new(TOKEN_BUCKET)
            .key(key)
            .arg(capacity)
            .arg(refill_per_sec)
            .arg(now)
            .arg(cost)
            .invoke_async(&mut c)
            .await
            .map_err(map_redis)?;
        Ok((
            allowed == 1,
            remaining.max(0) as u32,
            retry.parse().unwrap_or(0.0),
        ))
    }

    pub async fn ping(&self) -> Result<()> {
        let mut c = self.conn.clone();
        let _: String = redis::cmd("PING")
            .query_async(&mut c)
            .await
            .map_err(map_redis)?;
        Ok(())
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let mut c = self.conn.clone();
        let raw: Option<Vec<u8>> = c.get(key).await.map_err(map_redis)?;
        match raw {
            Some(b) => Ok(Some(serde_json::from_slice(&b).map_err(Error::internal)?)),
            None => Ok(None),
        }
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) -> Result<()> {
        let mut c = self.conn.clone();
        let raw = serde_json::to_vec(value).map_err(Error::internal)?;
        let _: () = c
            .set_ex(key, raw, ttl.as_secs().max(1))
            .await
            .map_err(map_redis)?;
        Ok(())
    }

    pub async fn set_flag(&self, key: &str, ttl: Duration) -> Result<()> {
        let mut c = self.conn.clone();
        let _: () = c
            .set_ex(key, 1u8, ttl.as_secs().max(1))
            .await
            .map_err(map_redis)?;
        Ok(())
    }

    /// Push a JSON value onto a capped list (newest first) and trim it to `cap` entries.
    pub async fn push_capped<T: Serialize>(&self, key: &str, value: &T, cap: usize) -> Result<()> {
        let raw = serde_json::to_string(value).map_err(Error::internal)?;
        let mut conn = self.conn.clone();
        let _: () = redis::pipe()
            .lpush(key, raw)
            .ltrim(key, 0, cap.saturating_sub(1) as isize)
            .query_async(&mut conn)
            .await
            .map_err(map_redis)?;
        Ok(())
    }

    /// Read up to `n` newest entries of a capped list.
    pub async fn list_json<T: DeserializeOwned>(&self, key: &str, n: usize) -> Result<Vec<T>> {
        let mut conn = self.conn.clone();
        let raws: Vec<String> = conn
            .lrange(key, 0, n.saturating_sub(1) as isize)
            .await
            .map_err(map_redis)?;
        Ok(raws
            .iter()
            .filter_map(|r| serde_json::from_str(r).ok())
            .collect())
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        let mut c = self.conn.clone();
        c.exists(key).await.map_err(map_redis)
    }

    pub async fn del(&self, key: &str) -> Result<()> {
        let mut c = self.conn.clone();
        let _: () = c.del(key).await.map_err(map_redis)?;
        Ok(())
    }

    pub async fn publish<T: Serialize>(&self, channel: &str, payload: &T) -> Result<()> {
        let mut c = self.conn.clone();
        let raw = serde_json::to_vec(payload).map_err(Error::internal)?;
        let _: () = c
            .publish(self.key(&["events", channel]), raw)
            .await
            .map_err(map_redis)?;
        Ok(())
    }
}

fn map_redis(e: redis::RedisError) -> Error {
    if e.is_connection_refusal() || e.is_io_error() || e.is_timeout() {
        Error::unavailable("cache unavailable")
    } else {
        Error::internal(e)
    }
}

/// A dedicated pub/sub subscription (Redis needs a separate connection for SUBSCRIBE).
pub struct Subscription {
    pubsub: redis::aio::PubSub,
}

impl Cache {
    pub async fn subscribe(&self, url: &str, channels: &[&str]) -> Result<Subscription> {
        let client = redis::Client::open(url).context("parsing redis url")?;
        let mut pubsub = client.get_async_pubsub().await.map_err(map_redis)?;
        for ch in channels {
            pubsub
                .subscribe(self.key(&["events", ch]))
                .await
                .map_err(map_redis)?;
        }
        Ok(Subscription { pubsub })
    }
}

impl Subscription {
    /// Next message as (channel suffix, JSON payload). Returns `None` when the connection drops.
    pub async fn next<T: DeserializeOwned>(&mut self) -> Option<(String, T)> {
        use futures_util::StreamExt;
        loop {
            let msg = self.pubsub.on_message().next().await?;
            let channel: String = msg
                .get_channel_name()
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_string();
            let payload: Vec<u8> = match msg.get_payload() {
                Ok(p) => p,
                Err(_) => continue,
            };
            match serde_json::from_slice(&payload) {
                Ok(v) => return Some((channel, v)),
                Err(e) => tracing::warn!(error = %e, "ignoring malformed bus message"),
            }
        }
    }
}
