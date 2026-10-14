//! Build provenance baked in by `build.rs`.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("ZANSHIN_BUILD_COMMIT");
pub const COMMIT_SHORT: &str = env!("ZANSHIN_BUILD_COMMIT_SHORT");
pub const BUILD_TIME_UNIX: &str = env!("ZANSHIN_BUILD_TIME");
pub const RUSTC: &str = env!("ZANSHIN_BUILD_RUSTC");
pub const TARGET: &str = env!("ZANSHIN_BUILD_TARGET");

/// `0.1.0 (abc123def456, 2026-10-14T15:00:00Z)` — what `zanshin --version` prints.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ZANSHIN_BUILD_COMMIT_SHORT"),
    ")\nbuilt: ",
    env!("ZANSHIN_BUILD_TIME"),
    " (unix)\nrustc: ",
    env!("ZANSHIN_BUILD_RUSTC"),
    "\ntarget: ",
    env!("ZANSHIN_BUILD_TARGET")
);

pub fn build_time() -> Option<chrono::DateTime<chrono::Utc>> {
    BUILD_TIME_UNIX
        .parse::<i64>()
        .ok()
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
}

pub fn json() -> serde_json::Value {
    serde_json::json!({
        "version": VERSION,
        "commit": COMMIT,
        "commit_short": COMMIT_SHORT,
        "built_at": build_time(),
        "rustc": RUSTC,
        "target": TARGET,
    })
}
