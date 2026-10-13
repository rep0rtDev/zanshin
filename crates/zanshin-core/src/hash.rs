//! Deterministic bucketing shared by rollouts, variants and experiments.

use xxhash_rust::xxh3::xxh3_64;

/// Number of buckets: percentages are expressed in basis points (0.01 %).
pub const BUCKETS: u32 = 10_000;

/// Map `(namespace, salt, subject)` to a stable bucket in `0..BUCKETS`.
///
/// The same subject lands in the same bucket for the same namespace+salt forever, and in an
/// independent bucket for a different namespace or salt — so rollouts of different flags are not
/// correlated unless they deliberately share a salt.
pub fn bucket(namespace: &str, salt: &str, subject: &str) -> u32 {
    let mut input = Vec::with_capacity(namespace.len() + salt.len() + subject.len() + 2);
    input.extend_from_slice(namespace.as_bytes());
    input.push(0);
    input.extend_from_slice(salt.as_bytes());
    input.push(0);
    input.extend_from_slice(subject.as_bytes());
    (xxh3_64(&input) % BUCKETS as u64) as u32
}

/// `true` when the subject falls inside the first `percentage` (0..=100, fractional ok) of buckets.
pub fn in_rollout(namespace: &str, salt: &str, subject: &str, percentage: f64) -> bool {
    if percentage >= 100.0 {
        return true;
    }
    if percentage <= 0.0 {
        return false;
    }
    let threshold = (percentage / 100.0 * BUCKETS as f64).round() as u32;
    bucket(namespace, salt, subject) < threshold
}

/// Pick an index from `weights` proportionally to the weights using the subject's bucket.
/// Returns `None` when all weights are zero.
pub fn weighted_pick(namespace: &str, salt: &str, subject: &str, weights: &[f64]) -> Option<usize> {
    let total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    if total <= 0.0 {
        return None;
    }
    let point = bucket(namespace, salt, subject) as f64 / BUCKETS as f64 * total;
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        if *w <= 0.0 {
            continue;
        }
        acc += w;
        if point < acc {
            return Some(i);
        }
    }
    weights.iter().rposition(|w| *w > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_is_stable_and_in_range() {
        let a = bucket("flag", "s", "player-1");
        assert_eq!(a, bucket("flag", "s", "player-1"));
        assert!(a < BUCKETS);
        assert_ne!(
            bucket("flag", "s", "player-1"),
            bucket("other", "s", "player-1")
        );
    }

    #[test]
    fn rollout_distribution_is_roughly_uniform() {
        let n = 20_000;
        let hits = (0..n)
            .filter(|i| in_rollout("f", "", &format!("u{i}"), 25.0))
            .count();
        let ratio = hits as f64 / n as f64;
        assert!((0.23..0.27).contains(&ratio), "ratio {ratio}");
        assert!(in_rollout("f", "", "x", 100.0));
        assert!(!in_rollout("f", "", "x", 0.0));
    }

    #[test]
    fn weighted_pick_respects_weights() {
        let n = 30_000;
        let mut counts = [0usize; 3];
        for i in 0..n {
            counts[weighted_pick("exp", "", &format!("u{i}"), &[70.0, 20.0, 10.0]).unwrap()] += 1;
        }
        let f = |c: usize| c as f64 / n as f64;
        assert!((0.68..0.72).contains(&f(counts[0])));
        assert!((0.18..0.22).contains(&f(counts[1])));
        assert!((0.08..0.12).contains(&f(counts[2])));
        assert_eq!(weighted_pick("e", "", "u", &[0.0, 0.0]), None);
        assert_eq!(weighted_pick("e", "", "u", &[0.0, 5.0]), Some(1));
    }
}
