//! The residual: the virtual channel `t_i − (i / f + t0)` of a timestamped
//! stream, in seconds. `i` is the raw sample index, so jitter shows as noise,
//! each dropped sample as a step of `1/f`, and clock drift as a slope against
//! the nominal rate that vanishes when the fitted rate is used instead.

use crate::ratefit::RateFit;

/// How a stream's stamps are compared against a line of rate `f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residual {
    first_stamp: i64,
    seconds_per_tick: f64,
    rate_hz: f64,
    t0_s: f64,
}

impl Residual {
    /// Against the nominal rate, from the first stamp: zero at sample 0.
    pub fn against_nominal(first_stamp: i64, seconds_per_tick: f64, nominal_hz: f64) -> Residual {
        Residual {
            first_stamp,
            seconds_per_tick,
            rate_hz: nominal_hz,
            t0_s: 0.0,
        }
    }

    /// Against the fitted line, so what is left is jitter and drops.
    pub fn against_fit(first_stamp: i64, seconds_per_tick: f64, fit: &RateFit) -> Residual {
        Residual {
            first_stamp,
            seconds_per_tick,
            rate_hz: fit.rate_hz,
            t0_s: fit.t0_s,
        }
    }

    /// The residual of sample `index`, stamped `stamp`, in seconds.
    pub fn at(&self, index: usize, stamp: i64) -> f64 {
        (stamp - self.first_stamp) as f64 * self.seconds_per_tick
            - (index as f64 / self.rate_hz + self.t0_s)
    }
}
