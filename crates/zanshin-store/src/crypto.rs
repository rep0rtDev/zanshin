//! Encryption at rest for secrets kept in Postgres: integration and export-sink configs, webhook
//! signing secrets and notification-channel URLs/headers.
//!
//! Values are sealed with AES-256-GCM under the *active* data key and stored as
//! `enc:v1:<kid>:<base64url(nonce || ciphertext)>`. Every key listed in `security.data_keys` can
//! open values; only the first one seals new ones, so rotation is "prepend a new key, run
//! `zanshin --rewrap-secrets`, drop the old key". Without a configured key the store falls back to
//! plaintext (and `--doctor` says so).

use std::sync::RwLock;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use zanshin_core::integrations::is_secret_key;

const PREFIX: &str = "enc:v1:";

struct DataKey {
    kid: String,
    key: LessSafeKey,
}

struct Keyring {
    keys: Vec<DataKey>,
}

static RING: RwLock<Option<Keyring>> = RwLock::new(None);

/// Parse `kid:base64(32 bytes)[,kid:base64...]`. The first entry is the active key.
pub fn parse(spec: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (kid, raw) = part
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("data key {part:?}: expected `kid:base64`"))?;
        let kid = kid.trim();
        if kid.is_empty()
            || kid.len() > 32
            || !kid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!("data key id {kid:?}: use 1-32 of [A-Za-z0-9_-]");
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .or_else(|_| B64.decode(raw.trim()))
            .map_err(|_| anyhow::anyhow!("data key {kid:?}: not valid base64"))?;
        if bytes.len() != 32 {
            anyhow::bail!(
                "data key {kid:?}: {} bytes, need exactly 32 (openssl rand -base64 32)",
                bytes.len()
            );
        }
        if out.iter().any(|(k, _): &(String, Vec<u8>)| k == kid) {
            anyhow::bail!("data key id {kid:?} listed twice");
        }
        out.push((kid.to_string(), bytes));
    }
    Ok(out)
}

/// Install the keyring for this process. `None`/empty disables encryption (plaintext at rest).
pub fn install(spec: Option<&str>) -> anyhow::Result<usize> {
    let parsed = match spec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => parse(s)?,
        None => Vec::new(),
    };
    let keys = parsed
        .into_iter()
        .map(|(kid, bytes)| {
            let key = UnboundKey::new(&AES_256_GCM, &bytes).expect("32-byte AES key");
            DataKey {
                kid,
                key: LessSafeKey::new(key),
            }
        })
        .collect::<Vec<_>>();
    let n = keys.len();
    *RING.write().expect("keyring lock") = if n == 0 { None } else { Some(Keyring { keys }) };
    Ok(n)
}

pub fn enabled() -> bool {
    RING.read().expect("keyring lock").is_some()
}

pub fn active_kid() -> Option<String> {
    RING.read()
        .expect("keyring lock")
        .as_ref()
        .map(|r| r.keys[0].kid.clone())
}

pub fn is_sealed(s: &str) -> bool {
    s.starts_with(PREFIX)
}

/// Encrypt a secret string. Already-sealed values and plaintext without a keyring pass through.
pub fn seal_str(plain: &str) -> String {
    if plain.is_empty() || is_sealed(plain) {
        return plain.to_string();
    }
    let guard = RING.read().expect("keyring lock");
    let Some(ring) = guard.as_ref() else {
        return plain.to_string();
    };
    let k = &ring.keys[0];
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).expect("os rng");
    let mut buf = plain.as_bytes().to_vec();
    k.key
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(k.kid.as_bytes()),
            &mut buf,
        )
        .expect("aes-gcm seal");
    let mut body = nonce.to_vec();
    body.extend_from_slice(&buf);
    format!("{PREFIX}{}:{}", k.kid, B64.encode(body))
}

/// Decrypt a value written by [`seal_str`]; plaintext (legacy or unencrypted deployments) is
/// returned unchanged. Fails when the key id is not in the keyring or the ciphertext is corrupt.
pub fn open_str(s: &str) -> anyhow::Result<String> {
    let Some(rest) = s.strip_prefix(PREFIX) else {
        return Ok(s.to_string());
    };
    let (kid, b64) = rest
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("malformed sealed secret"))?;
    let guard = RING.read().expect("keyring lock");
    let key = guard
        .as_ref()
        .and_then(|r| r.keys.iter().find(|k| k.kid == kid))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "secret is sealed with data key {kid:?}, which is not in security.data_keys"
            )
        })?;
    let body = B64
        .decode(b64)
        .map_err(|_| anyhow::anyhow!("sealed secret: bad base64"))?;
    if body.len() < NONCE_LEN + AES_256_GCM.tag_len() {
        anyhow::bail!("sealed secret: too short");
    }
    let (nonce, ct) = body.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).expect("12-byte nonce");
    let mut buf = ct.to_vec();
    let plain = key
        .key
        .open_in_place(nonce, Aad::from(kid.as_bytes()), &mut buf)
        .map_err(|_| {
            anyhow::anyhow!(
                "sealed secret: authentication failed (wrong key {kid:?} or corrupt data)"
            )
        })?;
    Ok(String::from_utf8(plain.to_vec())?)
}

/// True when the value should be rewritten by `--rewrap-secrets`: plaintext while a keyring is
/// installed, or sealed under a key other than the active one.
pub fn needs_rewrap(s: &str) -> bool {
    let Some(active) = active_kid() else {
        return false;
    };
    match s.strip_prefix(PREFIX).and_then(|r| r.split_once(':')) {
        Some((kid, _)) => kid != active,
        None => !s.is_empty(),
    }
}

fn walk(
    v: &Value,
    f: &dyn Fn(&str) -> anyhow::Result<String>,
    under_secret: bool,
) -> anyhow::Result<Value> {
    Ok(match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), walk(v, f, is_secret_key(k))?)))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|v| walk(v, f, under_secret))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::String(s) if under_secret => Value::String(f(s)?),
        other => other.clone(),
    })
}

/// Seal every string under a secret-looking key (see `zanshin_core::integrations::is_secret_key`)
/// anywhere in a config object.
pub fn seal_value(v: &Value) -> Value {
    walk(v, &|s| Ok(seal_str(s)), false).expect("seal never fails")
}

pub fn open_value(v: &Value) -> anyhow::Result<Value> {
    walk(v, &open_str, false)
}

pub fn value_needs_rewrap(v: &Value) -> bool {
    fn any(v: &Value, under_secret: bool) -> bool {
        match v {
            Value::Object(m) => m.iter().any(|(k, v)| any(v, is_secret_key(k))),
            Value::Array(a) => a.iter().any(|v| any(v, under_secret)),
            Value::String(s) if under_secret => needs_rewrap(s),
            _ => false,
        }
    }
    any(v, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const K1: &str = "k1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const K2: &str = "k2:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqo=";

    #[test]
    fn roundtrip_rotation_and_legacy_plaintext() {
        install(None).unwrap();
        assert!(!enabled());
        assert_eq!(seal_str("hunter2"), "hunter2");
        assert_eq!(open_str("hunter2").unwrap(), "hunter2");

        install(Some(K1)).unwrap();
        let sealed = seal_str("hunter2");
        assert!(sealed.starts_with("enc:v1:k1:"));
        assert_eq!(seal_str(&sealed), sealed, "sealing is idempotent");
        assert_eq!(open_str(&sealed).unwrap(), "hunter2");
        assert_eq!(open_str("legacy-plain").unwrap(), "legacy-plain");
        assert!(needs_rewrap("legacy-plain"));
        assert!(!needs_rewrap(&sealed));
        assert!(!needs_rewrap(""));

        let cfg = json!({"bucket": "b", "secret_key": "s3cr3t", "nested": {"api_key": "x", "n": 1}, "list": [{"token": "t"}]});
        let sealed_cfg = seal_value(&cfg);
        assert_eq!(sealed_cfg["bucket"], "b");
        assert!(sealed_cfg["secret_key"]
            .as_str()
            .unwrap()
            .starts_with("enc:v1:k1:"));
        assert!(sealed_cfg["nested"]["api_key"]
            .as_str()
            .unwrap()
            .starts_with("enc:v1:"));
        assert!(sealed_cfg["list"][0]["token"]
            .as_str()
            .unwrap()
            .starts_with("enc:v1:"));
        assert_eq!(sealed_cfg["nested"]["n"], 1);
        assert_eq!(open_value(&sealed_cfg).unwrap(), cfg);
        assert!(!value_needs_rewrap(&sealed_cfg));

        // rotate: k2 active, k1 still opens
        install(Some(&format!("{K2},{K1}"))).unwrap();
        assert_eq!(open_str(&sealed).unwrap(), "hunter2");
        assert!(needs_rewrap(&sealed));
        assert!(value_needs_rewrap(&sealed_cfg));
        let re = seal_str(&open_str(&sealed).unwrap());
        assert!(re.starts_with("enc:v1:k2:"));

        // k1 dropped → clear error
        install(Some(K2)).unwrap();
        let err = open_str(&sealed).unwrap_err().to_string();
        assert!(err.contains("k1"), "{err}");
        // tamper
        let mut bad = re.clone();
        bad.replace_range(bad.len() - 2.., "AA");
        assert!(open_str(&bad).is_err());

        assert!(parse("k1:short").is_err());
        assert!(parse("bad id!:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").is_err());
        assert!(parse(&format!("{K1},{K1}")).is_err());
        install(None).unwrap();
    }
}
