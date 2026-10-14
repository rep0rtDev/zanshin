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
