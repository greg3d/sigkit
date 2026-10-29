//! Fitted rate: a least-squares line through a stream's stamps.
//!
//! A fit against the raw sample index would be biased by dropped samples:
//! every drop shifts all later stamps by one interval, so a drop rate of one
//! in a thousand already reads as a thousand ppm. The fit therefore runs
//! against a gap-aware index: two consecutive stamps about two nominal
//! intervals apart count as two steps, and so on. Clock drift and jitter are
//! far smaller than one interval, so the rounding is unambiguous.

/// A least-squares line through (gap-aware index, stamp) pairs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateFit {
    /// Samples per second of the stream's clock.
    pub rate_hz: f64,
    /// Intercept of the fit, in seconds from the first stamp: the fitted time
    /// of index 0 (zero when the stamps lie exactly on the line).
    pub t0_s: f64,
    /// Samples the gap-aware index says are missing: gaps wider than one
    /// nominal interval, counted in whole intervals.
    pub missing: usize,
}

/// Fit `stamp_i · seconds_per_tick = t0 + index_i / rate` by least squares,
/// where `index_i` steps by `round(Δt · nominal_hz)` (at least 1) between
/// consecutive stamps. `None` with fewer than two stamps, a non-positive
/// nominal rate, or stamps that do not advance.
pub fn fit_rate(stamps: &[i64], seconds_per_tick: f64, nominal_hz: f64) -> Option<RateFit> {
    let n = stamps.len();
    if n < 2 || !(nominal_hz.is_finite() && nominal_hz > 0.0) {
        return None;
    }
    let first = stamps[0];
    let seconds = |stamp: i64| (stamp - first) as f64 * seconds_per_tick;

    let mut index = Vec::with_capacity(n);
    let mut last: u64 = 0;
    index.push(0u64);
    for pair in stamps.windows(2) {
        let steps = ((pair[1] - pair[0]) as f64 * seconds_per_tick * nominal_hz)
            .round()
            .max(1.0) as u64;
        last += steps;
        index.push(last);
    }
    let missing = (last + 1) as usize - n;

    let mean_i = index.iter().map(|&i| i as f64).sum::<f64>() / n as f64;
    let mean_t = stamps.iter().map(|&s| seconds(s)).sum::<f64>() / n as f64;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (&i, &s) in index.iter().zip(stamps) {
        let di = i as f64 - mean_i;
        let dt = seconds(s) - mean_t;
        sxy += di * dt;
        sxx += di * di;
    }
    let slope = sxy / sxx;
    if slope.is_nan() || slope <= 0.0 || slope.is_infinite() {
        return None;
    }
    Some(RateFit {
        rate_hz: 1.0 / slope,
        t0_s: mean_t - slope * mean_i,
        missing,
    })
}

/// Deviation of `rate_hz` from `nominal_hz` in parts per million.
pub fn ppm_from_nominal(rate_hz: f64, nominal_hz: f64) -> f64 {
    (rate_hz / nominal_hz - 1.0) * 1e6
}
