//! Seam: the residual channel `t_i − (i / f + t0)` of a timestamped stream and
//! the rate fit it is subtracted with, checked against the drift, jitter and
//! drops the generator injected.

use sigkit_format::synth::{self, Signal, SyntheticRecording, TimestampedSpec};
use sigkit_format::{Residual, fit_rate, ppm_from_nominal};

const NOMINAL_HZ: f64 = 416.0;

fn spec() -> TimestampedSpec {
    TimestampedSpec {
        name: "residual".into(),
        rate_nominal_hz: NOMINAL_HZ,
        duration_s: 20.0,
        jitter_s: 0.0,
        drop_fraction: 0.0,
        ppm: 50.0,
        signal: Signal::default(),
        seed: 11,
    }
}

fn stamps(made: &SyntheticRecording) -> &[i64] {
    made.streams[0].time.as_deref().unwrap()
}

fn residual_of(stamps: &[i64], residual: &Residual) -> Vec<f64> {
    stamps
        .iter()
        .enumerate()
        .map(|(i, &s)| residual.at(i, s))
        .collect()
}

/// Least-squares slope of `values` against their index.
fn slope(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let mean_i = (n - 1.0) / 2.0;
    let mean_v = values.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, v) in values.iter().enumerate() {
        sxy += (i as f64 - mean_i) * (v - mean_v);
        sxx += (i as f64 - mean_i).powi(2);
    }
    sxy / sxx
}

fn mean_and_sd(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

#[test]
fn against_nominal_a_drifting_clock_is_a_straight_slope() {
    let made = synth::timestamped(&spec());
    let stamps = stamps(&made);
    let residual = Residual::against_nominal(stamps[0], 1e-9, NOMINAL_HZ);
    let r = residual_of(stamps, &residual);

    assert_eq!(r[0], 0.0);
    // A clock 50 ppm fast stamps each sample at 1 / 1.00005 of its true
    // time: about -50 µs per second.
    let per_second = slope(&r) * NOMINAL_HZ;
    assert!(
        (per_second - (1.0 / (1.0 + 50e-6) - 1.0)).abs() < 1e-12,
        "{per_second} s per second"
    );
    // Straight: off the line by no more than the nanosecond rounding.
    for (i, v) in r.iter().enumerate() {
        let line = per_second * i as f64 / NOMINAL_HZ;
        assert!((v - line).abs() < 2e-9, "sample {i}: {v} vs {line}");
    }
}

#[test]
fn with_the_fit_subtracted_it_is_flat_noise_of_the_injected_jitter() {
    let jitter_s = 50e-6;
    let made = synth::timestamped(&TimestampedSpec { jitter_s, ..spec() });
    let stamps = stamps(&made);
    let fit = fit_rate(stamps, 1e-9, NOMINAL_HZ).unwrap();
    let r = residual_of(stamps, &Residual::against_fit(stamps[0], 1e-9, &fit));

    let (mean, sd) = mean_and_sd(&r);
    assert!(mean.abs() < 1e-9, "mean {mean}");
    assert!((sd / jitter_s - 1.0).abs() < 0.05, "sd {sd} vs {jitter_s}");
    // Flat: no drift left over the whole stream.
    let over_stream = slope(&r) * r.len() as f64;
    assert!(over_stream.abs() < jitter_s / 10.0, "{over_stream} s");
}

#[test]
fn each_dropped_sample_is_a_step_of_one_interval() {
    let made = synth::timestamped(&TimestampedSpec {
        ppm: 0.0,
        drop_fraction: 0.02,
        ..spec()
    });
    let stamps = stamps(&made);
    let r = residual_of(
        stamps,
        &Residual::against_nominal(stamps[0], 1e-9, NOMINAL_HZ),
    );
    let interval = 1.0 / NOMINAL_HZ;

    let mut steps = 0;
    for (i, pair) in stamps.windows(2).enumerate() {
        let dropped = ((pair[1] - pair[0]) as f64 * 1e-9 * NOMINAL_HZ).round() - 1.0;
        let step = r[i + 1] - r[i];
        assert!(
            (step - dropped * interval).abs() < 2e-9,
            "after sample {i}: step {step} s for {dropped} dropped"
        );
        steps += dropped as usize;
    }
    let fit = fit_rate(stamps, 1e-9, NOMINAL_HZ).unwrap();
    assert!(steps > 100, "{steps} drops");
    assert_eq!(steps, fit.missing);
    assert!((r[r.len() - 1] - steps as f64 * interval).abs() < 2e-9);
}

#[test]
fn the_fitted_ppm_matches_the_injected_drift_within_its_uncertainty() {
    for (ppm, jitter_s, drop_fraction) in [(50.0, 50e-6, 0.0), (-120.0, 200e-6, 0.03)] {
        let made = synth::timestamped(&TimestampedSpec {
            ppm,
            jitter_s,
            drop_fraction,
            ..spec()
        });
        let fit = fit_rate(stamps(&made), 1e-9, NOMINAL_HZ).unwrap();
        let fitted = ppm_from_nominal(fit.rate_hz, NOMINAL_HZ);
        let sd = fit.rate_sd_hz / NOMINAL_HZ * 1e6;
        assert!(sd > 0.0 && sd < 1.0, "uncertainty {sd} ppm");
        assert!(
            (fitted - ppm).abs() < 4.0 * sd,
            "fitted {fitted} ± {sd} ppm, injected {ppm}"
        );
    }
}
