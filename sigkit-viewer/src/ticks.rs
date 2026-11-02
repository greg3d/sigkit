//! Grid ticks: 1-2-5 steps on both axes, and the time axis unit.
//!
//! Steps are computed as `factor × 10^e` or `factor / 10^-e`, which keeps
//! them exact where the true value is representable, so labels read `0.5`
//! and not `0.5000000000000001`.

use crate::view::Span;

/// The smallest step of the form 1, 2 or 5 times a power of ten that is at
/// least `min_step`. Non-positive or non-finite input yields `1.0`.
pub fn step_125(min_step: f64) -> f64 {
    if !min_step.is_finite() || min_step <= 0.0 {
        return 1.0;
    }
    let mut exp = min_step.log10().floor() as i32;
    // log10 is not exact at powers of ten; settle the decade by comparison.
    if pow10(exp + 1) <= min_step {
        exp += 1;
    } else if pow10(exp) > min_step {
        exp -= 1;
    }
    let mantissa = min_step / pow10(exp);
    let factor = if mantissa <= 1.0 {
        1.0
    } else if mantissa <= 2.0 {
        2.0
    } else if mantissa <= 5.0 {
        5.0
    } else {
        10.0
    };
    scaled(factor, exp)
}

fn pow10(exp: i32) -> f64 {
    scaled(1.0, exp)
}

/// `factor × 10^exp`, exact where representable.
fn scaled(factor: f64, exp: i32) -> f64 {
    if exp >= 0 {
        factor * 10f64.powi(exp)
    } else {
        factor / 10f64.powi(-exp)
    }
}

/// The multiples of `step` inside `[from, to]`, both ends included.
pub fn ticks(from: f64, to: f64, step: f64) -> Vec<f64> {
    if !step.is_finite() || step <= 0.0 || !(from.is_finite() && to.is_finite()) || to < from {
        return Vec::new();
    }
    let first = (from / step).ceil();
    let last = (to / step).floor();
    if last < first || last - first > 10_000.0 {
        return Vec::new();
    }
    ((first as i64)..=(last as i64))
        .map(|k| k as f64 * step)
        .collect()
}

/// `value` with the decimals its grid `step` needs and no trailing zeros:
/// `1.5`, `1500`, `0.003`, never `-0`.
pub fn format_value(value: f64, step: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    let decimals = if step > 0.0 && step.is_finite() {
        (-step.log10().floor()).max(0.0) as usize
    } else {
        0
    };
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// The unit the time axis labels in. Steps of 1 ms .. 500 ms label in ms,
/// 1 s .. 20 s in s, from 1 min up in minutes (1, 2, 5, 10, 20, 50 min). A
/// millisecond step far from the zero (10 s or more) labels in seconds with
/// millisecond decimals, so labels read `1800.001 s`, not `1800001 ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Ms,
    S,
    Min,
}

impl TimeUnit {
    pub fn suffix(self) -> &'static str {
        match self {
            TimeUnit::Ms => "ms",
            TimeUnit::S => "s",
            TimeUnit::Min => "min",
        }
    }

    /// `seconds` expressed in this unit.
    pub fn scale(self, seconds: f64) -> f64 {
        match self {
            TimeUnit::Ms => seconds * 1e3,
            TimeUnit::S => seconds,
            TimeUnit::Min => seconds / 60.0,
        }
    }
}

/// The time axis grid: a 1-2-5 step in its unit, held in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAxis {
    pub unit: TimeUnit,
    pub step_s: f64,
}

impl TimeAxis {
    /// The label of the tick at `t_s` seconds: `1500 ms`, `40 s`, `4 min`.
    pub fn label(&self, t_s: f64) -> String {
        let value = self.unit.scale(t_s);
        let step = self.unit.scale(self.step_s);
        format!("{} {}", format_value(value, step), self.unit.suffix())
    }
}

/// Beyond this many seconds from the zero, a millisecond grid labels in seconds.
const MS_LABEL_LIMIT_S: f64 = 10.0;

/// Choose the time grid for the window `time` drawn over `px` pixels with at
/// least `min_px_per_tick` pixels between ticks.
pub fn time_axis(time: Span, px: f32, min_px_per_tick: f32) -> TimeAxis {
    let max_ticks = (px / min_px_per_tick).max(1.0) as f64;
    let min_step_s = time.len() / max_ticks;
    let step_s = step_125(min_step_s);
    if step_s > 20.0 {
        TimeAxis {
            unit: TimeUnit::Min,
            step_s: step_125(min_step_s / 60.0) * 60.0,
        }
    } else if step_s >= 1.0 || time.from.abs().max(time.to.abs()) >= MS_LABEL_LIMIT_S {
        TimeAxis {
            unit: TimeUnit::S,
            step_s,
        }
    } else {
        TimeAxis {
            unit: TimeUnit::Ms,
            step_s,
        }
    }
}

/// Choose the value grid step for `span` drawn over `px` pixels with at least
/// `min_px_per_tick` pixels between ticks.
pub fn value_step(span: f64, px: f32, min_px_per_tick: f32) -> f64 {
    let max_ticks = (px / min_px_per_tick).max(1.0) as f64;
    step_125(span / max_ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_1_2_or_5_times_a_power_of_ten_and_never_below_the_minimum() {
        assert_eq!(step_125(0.3), 0.5);
        assert_eq!(step_125(0.5), 0.5);
        assert_eq!(step_125(0.51), 1.0);
        assert_eq!(step_125(3.0), 5.0);
        assert_eq!(step_125(6.0), 10.0);
        assert_eq!(step_125(0.0012), 0.002);
        assert_eq!(step_125(25.0), 50.0);
        assert_eq!(step_125(1e6), 1e6);
    }

    #[test]
    fn ticks_are_the_multiples_of_the_step_inside_the_range_ends_included() {
        assert_eq!(ticks(-0.3, 1.2, 0.5), vec![0.0, 0.5, 1.0]);
        assert_eq!(ticks(0.0, 1.0, 0.5), vec![0.0, 0.5, 1.0]);
        assert_eq!(ticks(0.7, 0.9, 0.5), Vec::<f64>::new());
        assert_eq!(ticks(-1.0, -0.4, 0.5), vec![-1.0, -0.5]);
    }

    #[test]
    fn value_labels_carry_the_decimals_of_the_step_and_no_noise() {
        assert_eq!(format_value(-0.30000000000000004, 0.1), "-0.3");
        assert_eq!(format_value(1500.0, 500.0), "1500");
        assert_eq!(format_value(0.0, 0.5), "0");
        assert_eq!(format_value(-0.0, 0.5), "0");
        assert_eq!(format_value(0.003, 0.001), "0.003");
        assert_eq!(format_value(2.5, 0.5), "2.5");
    }

    /// 600 px with at least 80 px between ticks allows 7.5 ticks.
    fn axis(span_s: f64) -> TimeAxis {
        time_axis(Span::new(0.0, span_s), 600.0, 80.0)
    }

    #[test]
    fn far_from_the_zero_a_millisecond_grid_labels_in_seconds_with_millisecond_decimals() {
        let a = time_axis(Span::new(1800.0, 1800.003), 600.0, 80.0);
        assert_eq!(a.step_s, 0.0005);
        assert_eq!(a.label(1800.001), "1800.001 s");
        assert_eq!(a.label(1800.0015), "1800.0015 s");
        // Close to the zero the same step still reads in ms.
        let a = time_axis(Span::new(1.0, 1.003), 600.0, 80.0);
        assert_eq!(a.label(1.0015), "1001.5 ms");
        // A window reaching back before the zero counts its magnitude too.
        let a = time_axis(Span::new(-20.0, -19.0), 600.0, 80.0);
        assert_eq!(a.label(-19.8), "-19.8 s");
    }

    #[test]
    fn the_time_axis_steps_in_1_2_5_and_switches_between_ms_s_and_min() {
        let a = axis(3.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::Ms, 0.5));
        assert_eq!(a.label(1.5), "1500 ms");

        let a = axis(0.002);
        assert_eq!((a.unit, a.step_s), (TimeUnit::Ms, 0.0005));
        assert_eq!(a.label(0.0015), "1.5 ms");

        let a = axis(90.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::S, 20.0));
        assert_eq!(a.label(40.0), "40 s");

        let a = axis(10.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::S, 2.0));
        assert_eq!(a.label(0.0), "0 s");

        let a = axis(600.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::Min, 120.0));
        assert_eq!(a.label(240.0), "4 min");

        let a = axis(3600.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::Min, 600.0));
        assert_eq!(a.label(1800.0), "30 min");
    }

    #[test]
    fn a_minute_step_is_1_2_5_in_minutes_not_in_seconds() {
        // 100 s / 7.5 = 13.3 s per tick → 20 s, still seconds. 300 s / 7.5 = 40 s
        // → 50 s in seconds, but 50 s is not a minute step: 1 min.
        let a = axis(300.0);
        assert_eq!((a.unit, a.step_s), (TimeUnit::Min, 60.0));
        assert_eq!(axis(100.0).step_s, 20.0);
    }
}
