//! Seam: the least-squares rate fit over a stream's stamps, and the generator
//! it is verified against. The fit must recover the ppm drift the generator
//! injected, within the noise the generator adds, with and without dropped
//! samples.

use sigkit_format::ratefit::{fit_rate, ppm_from_nominal};
use sigkit_format::synth::{
    self, FixedRateSpec, PairSpec, Signal, SyntheticRecording, TimestampedSpec,
};
use sigkit_format::{Column, StreamKind};

#[test]
fn exact_stamps_fit_their_rate_exactly_whatever_the_clock_offset() {
    let base = 1_234_567_890_123_456i64;
    let stamps: Vec<i64> = (0..1000).map(|i| base + i * 10_000_000).collect();
    let fit = fit_rate(&stamps, 1e-9, 100.0).unwrap();
    assert!((fit.rate_hz - 100.0).abs() < 1e-9, "rate {}", fit.rate_hz);
    assert!(fit.t0_s.abs() < 1e-12, "t0 {}", fit.t0_s);
    assert_eq!(fit.missing, 0);
}

#[test]
fn a_gap_of_whole_intervals_counts_as_missing_samples_and_does_not_bend_the_fit() {
    let mut stamps: Vec<i64> = (0..1000).map(|i| i * 10_000_000).collect();
    stamps.drain(400..403);
    let fit = fit_rate(&stamps, 1e-9, 100.0).unwrap();
    assert!((fit.rate_hz - 100.0).abs() < 1e-9, "rate {}", fit.rate_hz);
    assert_eq!(fit.missing, 3);
}

#[test]
fn fewer_than_two_stamps_have_no_rate() {
    assert!(fit_rate(&[], 1e-9, 100.0).is_none());
    assert!(fit_rate(&[5], 1e-9, 100.0).is_none());
}

#[test]
fn ppm_from_nominal_is_relative_in_parts_per_million() {
    assert!((ppm_from_nominal(416.0624, 416.0) - 150.0).abs() < 1e-6);
    assert!((ppm_from_nominal(100.0, 100.0)).abs() < 1e-12);
    assert!((ppm_from_nominal(99.99, 100.0) + 100.0).abs() < 1e-6);
}

fn timestamped_spec() -> TimestampedSpec {
    TimestampedSpec {
        name: "drift".into(),
        rate_nominal_hz: 416.0,
        duration_s: 10.0,
        jitter_s: 50e-6,
        drop_fraction: 0.0,
        ppm: 150.0,
        signal: Signal::default(),
        seed: 7,
    }
}

fn fitted_ppm(made: &SyntheticRecording) -> (f64, usize) {
    let stream = &made.header.streams[0];
    let stamps = made.streams[0].time.as_ref().unwrap();
    let nominal = stream.rate_nominal_hz.unwrap();
    let fit = fit_rate(stamps, stream.seconds_per_tick().unwrap(), nominal).unwrap();
    (ppm_from_nominal(fit.rate_hz, nominal), fit.missing)
}

#[test]
fn the_fit_recovers_the_injected_ppm_drift_through_the_jitter() {
    let made = synth::timestamped(&timestamped_spec());
    let stream = &made.header.streams[0];
    assert_eq!(stream.kind, StreamKind::Timestamped);
    assert_eq!(stream.rate_nominal_hz, Some(416.0));
    assert_eq!(made.streams[0].time.as_ref().unwrap().len(), 4160);

    let (ppm, missing) = fitted_ppm(&made);
    assert!(
        (ppm - 150.0).abs() < 2.0,
        "fitted {ppm} ppm, injected 150 ppm"
    );
    assert_eq!(missing, 0);
}

#[test]
fn the_fit_recovers_the_injected_ppm_drift_in_spite_of_dropped_samples() {
    let made = synth::timestamped(&TimestampedSpec {
        drop_fraction: 0.05,
        ..timestamped_spec()
    });
    let kept = made.streams[0].time.as_ref().unwrap().len();
    assert!(kept < 4160, "{kept} samples");

    let (ppm, missing) = fitted_ppm(&made);
    assert!(
        (ppm - 150.0).abs() < 2.0,
        "fitted {ppm} ppm, injected 150 ppm"
    );
    assert_eq!(missing, 4160 - kept, "every drop is a missing sample");
}

#[test]
fn dropped_samples_shorten_the_stream_and_leave_gaps_of_whole_intervals() {
    let made = synth::timestamped(&TimestampedSpec {
        drop_fraction: 0.05,
        jitter_s: 0.0,
        ppm: 0.0,
        ..timestamped_spec()
    });
    let stamps = made.streams[0].time.as_ref().unwrap();
    assert!(
        stamps.len() < 4160 && stamps.len() > 3800,
        "{} samples",
        stamps.len()
    );
    assert_eq!(made.streams[0].channels[0].len(), stamps.len());
    let interval_ns = (1e9_f64 / 416.0).round() as i64;
    let mut gaps = 0;
    for pair in stamps.windows(2) {
        let d = pair[1] - pair[0];
        let k = ((d as f64) / (interval_ns as f64)).round() as i64;
        assert!(
            k >= 1 && (d - k * interval_ns).abs() <= 1,
            "gap {d} ns is not a whole number of intervals"
        );
        gaps += (k > 1) as usize;
    }
    assert!(gaps > 100, "{gaps} gaps");
}

#[test]
fn the_fixed_rate_preset_is_a_sine_at_the_rate_and_duration_asked_for() {
    let made = synth::fixed_rate(&FixedRateSpec {
        name: "sine".into(),
        rate_hz: 100.0,
        duration_s: 2.0,
        signal: Signal {
            freq_hz: 2.0,
            amplitude: 3.0,
            noise_sd: 0.0,
        },
        seed: 1,
    });
    assert_eq!(made.header.name, "sine");
    let stream = &made.header.streams[0];
    assert_eq!(
        (stream.kind, stream.rate_hz),
        (StreamKind::FixedRate, Some(100.0))
    );
    let Column::F32(values) = &made.streams[0].channels[0] else {
        panic!("expected f32");
    };
    assert_eq!(values.len(), 200);
    for (i, v) in values.iter().enumerate() {
        let want = 3.0 * (2.0 * std::f64::consts::PI * 2.0 * i as f64 / 100.0).sin();
        assert!((*v as f64 - want).abs() < 1e-6, "sample {i}: {v} vs {want}");
    }
}

#[test]
fn the_pair_preset_records_one_signal_on_two_clocks_with_a_known_shift() {
    let [a, b] = synth::pair(&PairSpec {
        name: "pair".into(),
        rate_nominal_hz: 100.0,
        duration_s: 4.0,
        jitter_s: 0.0,
        drop_fraction: 0.0,
        ppm_a: 0.0,
        ppm_b: 200.0,
        shift_s: 0.5,
        signal: Signal {
            freq_hz: 0.7,
            amplitude: 1.0,
            noise_sd: 0.0,
        },
        seed: 3,
    });
    assert_eq!(
        (a.header.name.as_str(), b.header.name.as_str()),
        ("pair-a", "pair-b")
    );

    // The same physical signal: b's sample j is a's sample j + shift·rate.
    let (Column::F32(va), Column::F32(vb)) = (&a.streams[0].channels[0], &b.streams[0].channels[0])
    else {
        panic!("expected f32");
    };
    for j in 0..vb.len() - 50 {
        assert!(
            (vb[j] - va[j + 50]).abs() < 1e-6,
            "sample {j}: {} vs {}",
            vb[j],
            va[j + 50]
        );
    }

    // Each clock drifts as asked.
    assert!(fitted_ppm(&a).0.abs() < 0.5, "a: {} ppm", fitted_ppm(&a).0);
    assert!(
        (fitted_ppm(&b).0 - 200.0).abs() < 0.5,
        "b: {} ppm",
        fitted_ppm(&b).0
    );

    // The UTC anchors agree on wall-clock time: b's zero is shift_s after a's.
    let anchor = |g: &SyntheticRecording| g.header.streams[0].utc_anchor.clone().unwrap();
    assert_eq!(anchor(&a).mono, a.streams[0].time.as_ref().unwrap()[0]);
    assert_eq!(anchor(&b).mono, b.streams[0].time.as_ref().unwrap()[0]);
    assert_eq!(anchor(&b).utc_ns - anchor(&a).utc_ns, 500_000_000);
}
