//! API keys: `zk_<kind>_<random>` strings; only the SHA-256 digest is stored.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyKind {
    /// Used by game clients: may authenticate identities and act on their own session.
    Client,
    /// Used by trusted game servers / bridges: may act on behalf of any identity.
    Server,
    /// Full administrative access.
    Admin,
}

impl ApiKeyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiKeyKind::Client => "client",
            ApiKeyKind::Server => "server",
            ApiKeyKind::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "client" => Ok(ApiKeyKind::Client),
            "server" => Ok(ApiKeyKind::Server),
            "admin" => Ok(ApiKeyKind::Admin),
            other => Err(Error::invalid(format!("unknown api key kind {other:?}"))),
        }
    }

    fn tag(&self) -> &'static str {
        match self {
            ApiKeyKind::Client => "c",
            ApiKeyKind::Server => "s",
            ApiKeyKind::Admin => "a",
        }
    }
}

/// A freshly generated key: the plaintext is returned to the caller exactly once.
#[derive(Debug, Clone)]
pub struct GeneratedKey {
    pub plaintext: String,
    pub prefix: String,
    pub hash: Vec<u8>,
}

pub fn generate(kind: ApiKeyKind) -> GeneratedKey {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let body = URL_SAFE_NO_PAD.encode(bytes);
    let plaintext = format!("zk_{}_{}", kind.tag(), body);
    GeneratedKey {
        prefix: plaintext[..12].to_string(),
        hash: hash(&plaintext),
        plaintext,
    }
}

pub fn hash(plaintext: &str) -> Vec<u8> {
    Sha256::digest(plaintext.as_bytes()).to_vec()
}

/// Cheap syntactic check so obviously-wrong tokens never hit the database.
pub fn looks_like_key(s: &str) -> bool {
    s.len() >= 20
        && s.len() <= 128
        && s.starts_with("zk_")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_unique_and_hash_stable() {
        let a = generate(ApiKeyKind::Client);
        let b = generate(ApiKeyKind::Client);
        assert_ne!(a.plaintext, b.plaintext);
        assert_eq!(a.hash, hash(&a.plaintext));
        assert!(looks_like_key(&a.plaintext));
        assert!(a.plaintext.starts_with("zk_c_"));
        assert_eq!(a.prefix.len(), 12);
    }
}
