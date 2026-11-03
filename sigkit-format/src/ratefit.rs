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
    /// Standard error of `rate_hz`, from the scatter of the stamps about the
    /// line: the fit's uncertainty. Zero when the stamps lie exactly on it.
    pub rate_sd_hz: f64,
    /// Intercept of the fit, in seconds from the first stamp: the fitted time
    /// of index 0 (zero when the stamps lie exactly on the line).
    pub t0_s: f64,
    /// Samples the gap-aware index says are missing: gaps wider than one
    /// nominal interval, counted in whole intervals.
    pub missing: usize,
}

impl RateFit {
    /// The fitted rate's deviation from `nominal_hz`, in ppm.
    pub fn ppm(&self, nominal_hz: f64) -> f64 {
        ppm_from_nominal(self.rate_hz, nominal_hz)
    }

    /// The uncertainty of [`RateFit::ppm`]: one standard error, in ppm.
    pub fn ppm_sd(&self, nominal_hz: f64) -> f64 {
        self.rate_sd_hz / nominal_hz * 1e6
    }
}

/// Fit `stamp_i · seconds_per_tick = t0 + index_i / rate` by least squares,
/// where `index_i` steps by `round(Δt · nominal_hz)` (at least 1) between
/// consecutive stamps. `None` with fewer than two stamps, a non-positive
/// nominal rate, or stamps that do not advance.
pub fn fit_rate(stamps: &[i64], seconds_per_tick: f64, nominal_hz: f64) -> Option<RateFit> {
    fit_rate_by(stamps.len(), |i| stamps[i], seconds_per_tick, nominal_hz)
}

/// [`fit_rate`] over `n` stamps read through `stamp`, so a memory-mapped time
/// column is fitted without reading it into memory. Three passes, no
/// allocation.
pub fn fit_rate_by(
    n: usize,
    stamp: impl Fn(usize) -> i64,
    seconds_per_tick: f64,
    nominal_hz: f64,
) -> Option<RateFit> {
    if n < 2 || !(nominal_hz.is_finite() && nominal_hz > 0.0) {
        return None;
    }
    let stamp = &stamp;
    let first = stamp(0);
    // Every pass walks (gap-aware index, seconds from the first stamp).
    let points = || {
        let mut index: u64 = 0;
        let mut previous = first;
        (0..n).map(move |i| {
            let s = stamp(i);
            if i > 0 {
                index += ((s - previous) as f64 * seconds_per_tick * nominal_hz)
                    .round()
                    .max(1.0) as u64;
            }
            previous = s;
            (index as f64, (s - first) as f64 * seconds_per_tick)
        })
    };

    let (mut sum_i, mut sum_t, mut last) = (0.0, 0.0, 0.0);
    for (i, t) in points() {
        sum_i += i;
        sum_t += t;
        last = i;
    }
    let (mean_i, mean_t) = (sum_i / n as f64, sum_t / n as f64);
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, t) in points() {
        sxy += (i - mean_i) * (t - mean_t);
        sxx += (i - mean_i) * (i - mean_i);
    }
    let slope = sxy / sxx;
    if slope.is_nan() || slope <= 0.0 || slope.is_infinite() {
        return None;
    }
    let t0_s = mean_t - slope * mean_i;
    let squared_error: f64 = points().map(|(i, t)| (t - t0_s - slope * i).powi(2)).sum();
    // Standard error of the slope, carried to the rate: d(1/s) = ds / s².
    let slope_sd = if n > 2 {
        (squared_error / (n - 2) as f64 / sxx).sqrt()
    } else {
        0.0
    };
    Some(RateFit {
        rate_hz: 1.0 / slope,
        rate_sd_hz: slope_sd / (slope * slope),
        t0_s,
        missing: last as usize + 1 - n,
    })
}

/// Deviation of `rate_hz` from `nominal_hz` in parts per million.
pub fn ppm_from_nominal(rate_hz: f64, nominal_hz: f64) -> f64 {
    (rate_hz / nominal_hz - 1.0) * 1e6
}
