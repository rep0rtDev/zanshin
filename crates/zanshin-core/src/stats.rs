//! Experiment statistics: Bayesian comparison of conversion rates (Beta posteriors) and a
//! sample-ratio-mismatch check (chi-square), both dependency-free and deterministic.

use serde::Serialize;

/// Posterior summary of a conversion rate and its comparison with control.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversionStats {
    pub rate: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    /// P(variant rate > control rate); `None` for the control itself or without a control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prob_beats_control: Option<f64>,
    /// Expected relative uplift over control (posterior mean of (v - c) / c).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_uplift: Option<f64>,
}

/// Sample-ratio mismatch: observed assignment counts vs the configured weights.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Srm {
    pub chi2: f64,
    pub p_value: f64,
    /// `p < 0.001` — assignment is skewed, results should not be trusted.
    pub suspicious: bool,
}

const SAMPLES: usize = 20_000;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let u1 = self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
    /// Marsaglia–Tsang gamma(shape, 1).
    fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let u = self.uniform();
            return self.gamma(shape + 1.0) * u.powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = self.uniform();
            if u < 1.0 - 0.0331 * x * x * x * x || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }
    fn beta(&mut self, a: f64, b: f64) -> f64 {
        let x = self.gamma(a);
        let y = self.gamma(b);
        if x + y == 0.0 {
            0.0
        } else {
            x / (x + y)
        }
    }
}

fn beta_samples(successes: i64, trials: i64, seed: u64) -> Vec<f64> {
    let a = successes.max(0) as f64 + 1.0;
    let b = (trials - successes).max(0) as f64 + 1.0;
    let mut rng = Rng::new(seed);
    let mut v: Vec<f64> = (0..SAMPLES).map(|_| rng.beta(a, b)).collect();
    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
    v
}

/// Beta(1,1)-prior posterior for `successes / trials`, compared against the control's
/// posterior when given. Deterministic (seeded by the inputs).
pub fn conversion(successes: i64, trials: i64, control: Option<(i64, i64)>) -> ConversionStats {
    let v = beta_samples(
        successes,
        trials,
        0x5eed ^ (successes as u64) << 20 ^ trials as u64,
    );
    let idx = |q: f64| v[((SAMPLES - 1) as f64 * q) as usize];
    let rate = if trials > 0 {
        successes as f64 / trials as f64
    } else {
        0.0
    };
    let (prob, uplift) = match control {
        Some((cs, ct)) if ct > 0 => {
            let c = beta_samples(cs, ct, 0xc0de ^ (cs as u64) << 20 ^ ct as u64);
            // Samples are sorted; pair them through a second pass in shuffled order so the
            // comparison is between independent draws, not quantiles.
            let mut rng = Rng::new(0xabc ^ trials as u64 ^ (ct as u64) << 16);
            let mut wins = 0usize;
            let mut uplift = 0.0;
            for &vi in &v {
                let cj = c[(rng.next_u64() % SAMPLES as u64) as usize];
                if vi > cj {
                    wins += 1;
                }
                if cj > 0.0 {
                    uplift += (vi - cj) / cj;
                }
            }
            (
                Some(wins as f64 / SAMPLES as f64),
                Some(uplift / SAMPLES as f64),
            )
        }
        _ => (None, None),
    };
    ConversionStats {
        rate,
        ci_low: idx(0.025),
        ci_high: idx(0.975),
        prob_beats_control: prob,
        expected_uplift: uplift,
    }
}

fn ln_gamma(x: f64) -> f64 {
    // Lanczos approximation (g = 7, n = 9).
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + 7.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Regularized upper incomplete gamma Q(s, x) (Numerical Recipes gammq).
fn gamma_q(s: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if x < s + 1.0 {
        // series for P, Q = 1 - P
        let mut sum = 1.0 / s;
        let mut del = sum;
        let mut ap = s;
        for _ in 0..500 {
            ap += 1.0;
            del *= x / ap;
            sum += del;
            if del.abs() < sum.abs() * 1e-14 {
                break;
            }
        }
        1.0 - sum * (-x + s * x.ln() - ln_gamma(s)).exp()
    } else {
        // continued fraction for Q
        let mut b = x + 1.0 - s;
        let mut c = 1.0 / 1e-300;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..500 {
            let an = -(i as f64) * (i as f64 - s);
            b += 2.0;
            d = an * d + b;
            if d.abs() < 1e-300 {
                d = 1e-300;
            }
            c = b + an / c;
            if c.abs() < 1e-300 {
                c = 1e-300;
            }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-14 {
                break;
            }
        }
        (-x + s * x.ln() - ln_gamma(s)).exp() * h
    }
}

/// Always-valid (anytime) test of a conversion difference against control, via the mixture
/// sequential probability ratio test (mSPRT) with a normal mixing prior. Unlike a fixed-horizon
/// z-test, `p_value` can be inspected at any moment without inflating the false-positive rate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SequentialStats {
    /// Observed difference variant − control.
    pub delta: f64,
    pub z: f64,
    /// Always-valid p-value (1 / likelihood ratio, capped at 1).
    pub p_value: f64,
    /// `p_value < 0.05`.
    pub significant: bool,
    /// Relative lift over control, when control converts at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lift: Option<f64>,
}

/// mSPRT for two Bernoulli rates. `tau` is the prior standard deviation of the effect; `None`
/// uses `max(0.005, 0.1 × pooled rate)`.
pub fn sequential(
    successes: i64,
    trials: i64,
    control: (i64, i64),
    tau: Option<f64>,
) -> Option<SequentialStats> {
    let (cs, ct) = control;
    if trials <= 0 || ct <= 0 {
        return None;
    }
    let (n_v, n_c) = (trials as f64, ct as f64);
    let (p_v, p_c) = (successes as f64 / n_v, cs as f64 / n_c);
    let pooled = (successes + cs) as f64 / (n_v + n_c);
    let var = (pooled * (1.0 - pooled)).max(1e-12) * (1.0 / n_v + 1.0 / n_c);
    let tau = tau.unwrap_or_else(|| (0.1 * pooled).max(0.005));
    let tau2 = tau * tau;
    let delta = p_v - p_c;
    let z = delta / var.sqrt();
    // Λ = sqrt(var / (var + tau²)) · exp(tau² · delta² / (2 · var · (var + tau²)))
    let ln_lambda =
        0.5 * (var / (var + tau2)).ln() + tau2 * delta * delta / (2.0 * var * (var + tau2));
    let p_value = (-ln_lambda).exp().min(1.0);
    Some(SequentialStats {
        delta,
        z,
        p_value,
        significant: p_value < 0.05,
        lift: (p_c > 0.0).then_some(delta / p_c),
    })
}

/// Thompson-sampling weights: the share of posterior draws in which each arm has the highest
/// conversion rate, floored at `min_weight` percent and rescaled to sum to 100. Deterministic.
#[allow(clippy::needless_range_loop)]
pub fn thompson_weights(arms: &[(i64, i64)], min_weight: f64) -> Vec<f64> {
    let k = arms.len();
    if k == 0 {
        return vec![];
    }
    let draws: Vec<Vec<f64>> = arms
        .iter()
        .enumerate()
        .map(|(i, (s, t))| {
            let mut v = beta_samples(
                *s,
                *t,
                0x7a0b ^ (i as u64) << 40 ^ (*s as u64) << 20 ^ *t as u64,
            );
            // Shuffle the sorted samples so arms are compared on independent draws.
            let mut rng = Rng::new(0x51 ^ (i as u64 + 1) << 8 ^ *t as u64);
            for j in (1..v.len()).rev() {
                let r = (rng.next_u64() % (j as u64 + 1)) as usize;
                v.swap(j, r);
            }
            v
        })
        .collect();
    let mut wins = vec![0usize; k];
    for d in 0..SAMPLES {
        let mut best = 0;
        for i in 1..k {
            if draws[i][d] > draws[best][d] {
                best = i;
            }
        }
        wins[best] += 1;
    }
    let floor = min_weight.clamp(0.0, 100.0 / k as f64);
    let free = 100.0 - floor * k as f64;
    wins.iter()
        .map(|w| floor + free * (*w as f64 / SAMPLES as f64))
        .collect()
}

/// Posterior probability of each arm being the best, in arm order.
pub fn best_arm_probabilities(arms: &[(i64, i64)]) -> Vec<f64> {
    let k = arms.len() as f64;
    thompson_weights(arms, 0.0)
        .into_iter()
        .map(|w| if k > 0.0 { w / 100.0 } else { 0.0 })
        .collect()
}

/// CUPED (controlled-experiment using pre-experiment data): shrink each unit's outcome by
/// `theta × (covariate − mean covariate)`. Returns `(theta, adjusted values, variance reduction)`.
pub fn cuped(outcomes: &[f64], covariates: &[f64]) -> (f64, Vec<f64>, f64) {
    let n = outcomes.len().min(covariates.len());
    if n < 2 {
        return (0.0, outcomes.to_vec(), 0.0);
    }
    let mean = |v: &[f64]| v.iter().take(n).sum::<f64>() / n as f64;
    let (my, mx) = (mean(outcomes), mean(covariates));
    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for i in 0..n {
        let (dy, dx) = (outcomes[i] - my, covariates[i] - mx);
        cov += dy * dx;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    if var_x <= 0.0 || var_y <= 0.0 {
        return (0.0, outcomes.to_vec(), 0.0);
    }
    let theta = cov / var_x;
    let adjusted: Vec<f64> = (0..n)
        .map(|i| outcomes[i] - theta * (covariates[i] - mx))
        .collect();
    let ma = mean(&adjusted);
    let var_a: f64 = adjusted.iter().map(|a| (a - ma) * (a - ma)).sum();
    let reduction = (1.0 - var_a / var_y).clamp(0.0, 1.0);
    (theta, adjusted, reduction)
}

/// Chi-square goodness of fit of observed assignment counts against weights.
pub fn srm(groups: &[(f64, i64)]) -> Option<Srm> {
    let total: i64 = groups.iter().map(|(_, n)| n).sum();
    let wsum: f64 = groups.iter().map(|(w, _)| w.max(0.0)).sum();
    if groups.len() < 2 || total < 50 || wsum <= 0.0 {
        return None;
    }
    let chi2: f64 = groups
        .iter()
        .map(|(w, n)| {
            let expected = total as f64 * w.max(0.0) / wsum;
            if expected <= 0.0 {
                0.0
            } else {
                (*n as f64 - expected).powi(2) / expected
            }
        })
        .sum();
    let dof = (groups.len() - 1) as f64;
    let p = gamma_q(dof / 2.0, chi2 / 2.0).clamp(0.0, 1.0);
    Some(Srm {
        chi2,
        p_value: p,
        suspicious: p < 0.001,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn sequential_and_bandit_and_cuped() {
        let none = super::sequential(50, 1000, (50, 1000), None).unwrap();
        assert!(!none.significant && none.p_value > 0.5, "{none:?}");
        let big = super::sequential(300, 1000, (100, 1000), None).unwrap();
        assert!(big.significant && big.lift.unwrap() > 1.9, "{big:?}");
        let w = super::thompson_weights(&[(10, 1000), (100, 1000), (12, 1000)], 5.0);
        assert_eq!(w.len(), 3);
        assert!((w.iter().sum::<f64>() - 100.0).abs() < 1e-6);
        assert!(w[1] > 80.0 && w[0] >= 5.0 && w[2] >= 5.0, "{w:?}");
        let y: Vec<f64> = (0..200)
            .map(|i| (i % 7) as f64 + (i as f64) * 0.1)
            .collect();
        let x: Vec<f64> = (0..200).map(|i| (i as f64) * 0.1).collect();
        let (theta, adj, red) = super::cuped(&y, &x);
        assert!(
            theta > 0.9 && theta < 1.1 && red > 0.5 && adj.len() == 200,
            "{theta} {red}"
        );
    }

    use super::*;

    #[test]
    fn conversion_posteriors() {
        let s = conversion(120, 1000, Some((100, 1000)));
        assert!((s.rate - 0.12).abs() < 1e-9);
        assert!(s.ci_low < 0.12 && 0.12 < s.ci_high);
        let p = s.prob_beats_control.unwrap();
        assert!(p > 0.85 && p < 0.99, "{p}");
        let same = conversion(100, 1000, Some((100, 1000)));
        let p = same.prob_beats_control.unwrap();
        assert!((p - 0.5).abs() < 0.03, "{p}");
        let none = conversion(0, 0, None);
        assert_eq!(none.rate, 0.0);
        assert!(none.prob_beats_control.is_none());
    }

    #[test]
    fn srm_detects_skew() {
        let ok = srm(&[(1.0, 5020), (1.0, 4980)]).unwrap();
        assert!(!ok.suspicious && ok.p_value > 0.5, "{ok:?}");
        let bad = srm(&[(1.0, 6000), (1.0, 4000)]).unwrap();
        assert!(bad.suspicious && bad.p_value < 1e-6, "{bad:?}");
        assert!(srm(&[(1.0, 10)]).is_none());
        // chi-square tail sanity: Q(0.5, x/2) for chi2=3.84, dof=1 → p≈0.05
        let p = gamma_q(0.5, 3.841 / 2.0);
        assert!((p - 0.05).abs() < 0.002, "{p}");
    }
}
