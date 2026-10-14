//! Embeds build provenance (git commit, build time, rustc) into the binary for `--version`,
//! `/` and `/admin/v1/system`. Release builds pass the values via env so container builds without
//! a `.git` directory still carry them.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=ZANSHIN_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=ZANSHIN_BUILD_TIME");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let commit = std::env::var("ZANSHIN_BUILD_COMMIT")
        .ok()
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let time = std::env::var("ZANSHIN_BUILD_TIME").ok().unwrap_or_else(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        secs.to_string()
    });
    let rustc = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=ZANSHIN_BUILD_COMMIT={commit}");
    println!(
        "cargo:rustc-env=ZANSHIN_BUILD_COMMIT_SHORT={}{}",
        &commit[..commit.len().min(12)],
        if dirty { "-dirty" } else { "" }
    );
    println!("cargo:rustc-env=ZANSHIN_BUILD_TIME={time}");
    println!("cargo:rustc-env=ZANSHIN_BUILD_RUSTC={rustc}");
    println!(
        "cargo:rustc-env=ZANSHIN_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
