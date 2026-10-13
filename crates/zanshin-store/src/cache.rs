//! Redis-backed cache and pub/sub bus.

use std::time::Duration;

use anyhow::Context;
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{de::DeserializeOwned, Serialize};
use zanshin_core::{Error, Result};

use crate::RedisConfig;

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
