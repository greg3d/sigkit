//! Seam: a recording's timestamped streams read as time. Clock domains are
//! converted on read to seconds from the recording's zero; a time column
//! must be monotonic; the residual channel gets a pyramid binned by time like
//! any other channel, with the stream's holes as gaps.

use sigkit_format::{
    BASE_BIN_INTERVALS, Channel, ClockDomain, Column, Dtype, Error, Header, Recording, Residual,
    Stream, StreamData, fit_rate,
};

fn stream(id: &str, domain: &str, unit: &str, rate: f64) -> Stream {
    let mut s = Stream::timestamped(id, ClockDomain::new(domain, unit), rate);
    s.channels.push(Channel::new("v", Dtype::F32, ""));
    s
}

fn data(stamps: Vec<i64>) -> StreamData {
    let values = (0..stamps.len()).map(|i| i as f32).collect();
    StreamData::timestamped(stamps, vec![Column::F32(values)])
}

#[test]
fn each_stream_starts_at_its_offset_in_seconds_from_its_clock_domains_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = Header::new("zero");
    header.streams = vec![
        // Android, in ns, first stamp at 10 s of uptime.
        stream("accel", "android.elapsedRealtimeNanos", "ns", 100.0),
        // The same clock written in µs, first stamp at 7.5 s: the zero.
        stream("gyro", "android.elapsedRealtimeNanos", "us", 100.0),
        // Another clock domain has a zero of its own.
        stream("gps", "ios.systemUptime", "ms", 1.0),
        Stream::fixed_rate("legacy", 50.0),
    ];
    header.streams[3]
        .channels
        .push(Channel::new("v", Dtype::F32, ""));
    let rec = Recording::create(
        tmp.path().join("z.sigrec"),
        header,
        &[
            data(vec![10_000_000_000, 10_010_000_000]),
            data(vec![7_500_000, 7_510_000]),
            data(vec![123_456_789, 123_457_789]),
            StreamData::fixed_rate(vec![Column::F32(vec![0.0; 4])]),
        ],
    )
    .unwrap();

    assert_eq!(rec.start_s("accel").unwrap(), 2.5);
    assert_eq!(rec.start_s("gyro").unwrap(), 0.0);
    assert_eq!(rec.start_s("gps").unwrap(), 0.0);
    assert_eq!(rec.start_s("legacy").unwrap(), 0.0);
}

#[test]
fn a_time_column_that_runs_backwards_is_an_error_naming_the_sample() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = Header::new("back");
    header
        .streams
        .push(stream("s", "sigkit.synthetic", "ns", 100.0));
    let rec = Recording::create(
        tmp.path().join("b.sigrec"),
        header,
        &[data(vec![
            0, 10_000_000, 20_000_000, 15_000_000, 40_000_000,
        ])],
    )
    .unwrap();

    let err = rec.pyramid("s", "v").unwrap_err();
    assert!(matches!(err, Error::InvalidStream { .. }), "{err}");
    assert!(err.to_string().contains("sample 3"), "{err}");
}

/// 100 Hz stamps in ns with a 2-second hole after sample 1000, a drift of
/// 1 µs per sample and no jitter.
fn holey_stamps() -> Vec<i64> {
    (0..3000i64)
        .map(|i| {
            let i = if i >= 1000 { i + 200 } else { i };
            5_000_000_000 + i * 10_001_000
        })
        .collect()
}

#[test]
fn the_rate_fit_reads_the_mapped_time_column() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = Header::new("fit");
    header
        .streams
        .push(stream("s", "sigkit.synthetic", "ns", 100.0));
    let stamps = holey_stamps();
    let rec =
        Recording::create(tmp.path().join("f.sigrec"), header, &[data(stamps.clone())]).unwrap();

    let fit = rec.fit_rate("s").unwrap().unwrap();
    assert_eq!(Some(fit), fit_rate(&stamps, 1e-9, 100.0));
    assert_eq!(fit.missing, 200);
}

#[test]
fn the_residual_pyramid_is_binned_by_time_with_the_holes_as_gaps() {
    let tmp = tempfile::tempdir().unwrap();
    let mut header = Header::new("res");
    header
        .streams
        .push(stream("s", "sigkit.synthetic", "ns", 100.0));
    let stamps = holey_stamps();
    let rec =
        Recording::create(tmp.path().join("r.sigrec"), header, &[data(stamps.clone())]).unwrap();
    let residual = Residual::against_nominal(stamps[0], 1e-9, 100.0);

    let pyramid = rec
        .residual_pyramid("s", &residual, &mut |_, _| {})
        .unwrap();
    let (channel, _) = rec.pyramid("s", "v").unwrap();
    assert_eq!(pyramid.base_bin_s(), channel.base_bin_s());
    assert_eq!(pyramid.level(0).len(), channel.level(0).len());

    // Brute force: sample i lies in base bin floor(Δt · 100 Hz / 64).
    let base = pyramid.level(0);
    for (k, bin) in base.iter().enumerate() {
        let values: Vec<f64> = (0..stamps.len())
            .filter(|&i| {
                let intervals = (stamps[i] - stamps[0]) * 100 / 1_000_000_000;
                intervals as usize / BASE_BIN_INTERVALS as usize == k
            })
            .map(|i| residual.at(i, stamps[i]))
            .collect();
        if values.is_empty() {
            assert!(bin.is_empty(), "bin {k} should be a gap");
            continue;
        }
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min) as f32;
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max) as f32;
        assert_eq!((bin.min, bin.max), (min, max), "bin {k}");
    }
    let gaps = base.iter().filter(|b| b.is_empty()).count();
    assert!(gaps >= 2, "{gaps} empty bins in a 2 s hole of 0.64 s bins");
}

#[test]
fn a_time_correction_needs_a_positive_k_and_a_finite_shift() {
    let mut s = stream("s", "sigkit.synthetic", "ns", 100.0);
    s.time_correction.k = 0.0;
    assert!(s.validate().is_err());
    s.time_correction.k = 1.0001;
    s.time_correction.shift_s = f64::NAN;
    assert!(s.validate().is_err());
    s.time_correction.shift_s = -3.0;
    assert!(s.validate().is_ok());
}

#[test]
fn a_generated_recording_moved_to_another_clock_keeps_its_times_in_seconds() {
    use sigkit_format::synth::{self, Signal, TimestampedSpec};
    let spec = TimestampedSpec {
        name: "ios".into(),
        rate_nominal_hz: 100.0,
        duration_s: 1.0,
        jitter_s: 0.0,
        drop_fraction: 0.0,
        ppm: 0.0,
        signal: Signal::default(),
        seed: 1,
    };
    let ns = synth::timestamped(&spec);
    let us = synth::timestamped(&spec).on_clock(ClockDomain::new("ios.systemUptime", "us"));

    let stream = &us.header.streams[0];
    assert_eq!(
        stream.time,
        Some(ClockDomain::new("ios.systemUptime", "us"))
    );
    let (a, b) = (
        ns.streams[0].time.as_ref().unwrap(),
        us.streams[0].time.as_ref().unwrap(),
    );
    assert_eq!(a.len(), b.len());
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a.div_euclid(1000), *b);
    }
    let anchor = stream.utc_anchor.as_ref().unwrap();
    assert_eq!(anchor.mono, b[0]);
    assert_eq!(
        anchor.utc_ns,
        ns.header.streams[0].utc_anchor.as_ref().unwrap().utc_ns
    );
}
