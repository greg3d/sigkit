//! Seam: the pyramid. Each bin's min and max equal the brute-force min and max
//! of the samples it covers, at every level; a bin covering no sample is
//! empty and stays a gap at every level; a single spike survives to the top.

use sigkit_format::pyramid::{BASE_BIN_INTERVALS, Pyramid};

/// Deterministic noisy samples with one spike at index 100_000.
fn samples(n: usize) -> Vec<f64> {
    let mut x: u64 = 0x1234_5678_9abc_def0;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let noise = (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            let base = (i as f64 * 0.001).sin();
            if i == 100_000 { 50.0 } else { base + noise }
        })
        .collect()
}

/// The min and max of `values[range]`, ignoring nothing: the reference.
fn brute_force(values: &[f64]) -> (f32, f32) {
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (min as f32, max as f32)
}

#[test]
fn every_bin_at_every_level_holds_the_brute_force_min_and_max_of_its_samples() {
    // Not a multiple of 64, so the last base bin is partial.
    let values = samples(150_001);
    let rate_hz = 500.0;
    let pyramid = Pyramid::build_fixed_rate(rate_hz, values.len(), |i| values[i]);

    assert_eq!(pyramid.base_bin_s(), BASE_BIN_INTERVALS as f64 / rate_hz);
    let base = BASE_BIN_INTERVALS as usize;
    assert_eq!(pyramid.level(0).len(), values.len().div_ceil(base));

    for level in 0..pyramid.levels() {
        let span = base << level;
        assert_eq!(pyramid.bin_s(level), span as f64 / rate_hz);
        for (k, bin) in pyramid.level(level).iter().enumerate() {
            let from = k * span;
            let to = (from + span).min(values.len());
            let (min, max) = brute_force(&values[from..to]);
            assert!(!bin.is_empty(), "level {level} bin {k} is empty");
            assert_eq!((bin.min, bin.max), (min, max), "level {level} bin {k}");
        }
    }
}

#[test]
fn the_top_level_is_one_bin_and_a_single_spike_is_still_its_max() {
    let values = samples(150_001);
    let pyramid = Pyramid::build_fixed_rate(500.0, values.len(), |i| values[i]);

    let top = pyramid.level(pyramid.levels() - 1);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].max, 50.0);
    assert_eq!(pyramid.whole_signal(), Some((top[0].min, 50.0)));
}

#[test]
fn a_bin_with_no_samples_is_empty_and_stays_a_gap_until_a_sibling_fills_it() {
    // Samples placed by bin: bins 0 and 1 hold two samples each, bins 2 and 3
    // are empty, bin 4 holds one sample, bins 5..7 are empty.
    let placed: Vec<(usize, f64)> = vec![(0, 1.0), (0, -1.0), (1, 2.0), (1, 0.5), (4, 7.0)];
    let pyramid = Pyramid::build(0.128, 8, placed.iter().copied());

    let base = pyramid.level(0);
    assert_eq!(base.len(), 8);
    assert_eq!((base[0].min, base[0].max), (-1.0, 1.0));
    assert_eq!((base[1].min, base[1].max), (0.5, 2.0));
    assert!(base[2].is_empty() && base[3].is_empty());
    assert_eq!((base[4].min, base[4].max), (7.0, 7.0));
    assert!(base[5].is_empty() && base[6].is_empty() && base[7].is_empty());

    // Level 1 merges pairs: (0,1) full, (2,3) still a gap, (4,5) takes bin 4, (6,7) gap.
    let l1 = pyramid.level(1);
    assert_eq!(l1.len(), 4);
    assert_eq!((l1[0].min, l1[0].max), (-1.0, 2.0));
    assert!(l1[1].is_empty());
    assert_eq!((l1[2].min, l1[2].max), (7.0, 7.0));
    assert!(l1[3].is_empty());

    // Level 2: (0..4) full, (4..8) has only the spike. Level 3: one bin, all.
    let l2 = pyramid.level(2);
    assert_eq!((l2[0].min, l2[0].max), (-1.0, 2.0));
    assert_eq!((l2[1].min, l2[1].max), (7.0, 7.0));
    assert_eq!(pyramid.levels(), 4);
    assert_eq!(pyramid.whole_signal(), Some((-1.0, 7.0)));
}

#[test]
fn an_empty_column_has_one_empty_base_bin_and_no_signal_range() {
    let pyramid = Pyramid::build_fixed_rate(100.0, 0, |_| unreachable!());
    assert_eq!(pyramid.levels(), 1);
    assert_eq!(pyramid.level(0).len(), 0);
    assert_eq!(pyramid.whole_signal(), None);
}

// ---- The recording seam: mapped columns and the cached pyramid ----

use sigkit_format::synth::{self, FixedRateSpec, Signal};
use sigkit_format::{
    Channel, ClockDomain, Column, Dtype, Header, Recording, Stream, StreamData,
    recording::PyramidSource,
};
use std::fs;
use std::time::{Duration, SystemTime};

fn fixed_rate_recording(dir: &std::path::Path, rate_hz: f64, duration_s: f64) -> Recording {
    synth::fixed_rate(&FixedRateSpec {
        name: "sine".into(),
        rate_hz,
        duration_s,
        signal: Signal::default(),
        seed: 3,
    })
    .write(dir)
    .unwrap()
}

#[test]
fn a_mapped_column_reads_the_same_samples_as_the_whole_column() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = fixed_rate_recording(&tmp.path().join("sine.sigrec"), 100.0, 3.0);
    let whole = rec
        .read_stream(synth::STREAM_ID)
        .unwrap()
        .channels
        .remove(0);
    let mapped = rec.map_column(synth::STREAM_ID, synth::CHANNEL_ID).unwrap();

    assert_eq!(mapped.dtype(), Dtype::F32);
    assert_eq!(mapped.len(), 300);
    assert_eq!(mapped.get(299), whole.get(299));
    assert_eq!(mapped.get(300), None);
    let mut out = Vec::new();
    mapped.read_into(10..20, &mut out);
    let expected: Vec<f64> = (10..20).map(|i| whole.get(i).unwrap()).collect();
    assert_eq!(out, expected);
}

#[test]
fn first_open_builds_the_pyramid_and_second_open_loads_the_same_one_from_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sine.sigrec");
    let built = {
        let rec = fixed_rate_recording(&dir, 500.0, 30.0);
        let (pyramid, source) = rec.pyramid(synth::STREAM_ID, synth::CHANNEL_ID).unwrap();
        assert_eq!(source, PyramidSource::Built);
        assert!(
            dir.join(".cache").is_dir(),
            "the cache lives inside the recording"
        );
        pyramid
    };
    let rec = Recording::open(&dir).unwrap();
    let (cached, source) = rec.pyramid(synth::STREAM_ID, synth::CHANNEL_ID).unwrap();
    assert_eq!(source, PyramidSource::Cache);
    assert_eq!(cached, built);
    assert_eq!(cached.base_bin_s(), 64.0 / 500.0);
}

#[test]
fn a_changed_column_invalidates_the_cache_and_the_pyramid_sees_the_new_samples() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sine.sigrec");
    let rec = fixed_rate_recording(&dir, 100.0, 10.0);
    let (before, _) = rec.pyramid(synth::STREAM_ID, synth::CHANNEL_ID).unwrap();
    assert!(before.whole_signal().unwrap().1 < 2.0);

    // Same length, new content: a spike in sample 0, and a later mtime.
    let stream = rec.header().stream(synth::STREAM_ID).unwrap();
    let path = dir.join(stream.channel_file_name(&stream.channels[0]));
    let mut bytes = fs::read(&path).unwrap();
    bytes[..4].copy_from_slice(&42.0f32.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    let later = SystemTime::now() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let rec = Recording::open(&dir).unwrap();
    let (after, source) = rec.pyramid(synth::STREAM_ID, synth::CHANNEL_ID).unwrap();
    assert_eq!(source, PyramidSource::Built);
    assert_eq!(after.whole_signal().unwrap().1, 42.0);
}

#[test]
fn a_timestamped_stream_is_binned_by_time_and_a_hole_in_the_stamps_is_a_gap() {
    let tmp = tempfile::tempdir().unwrap();
    // 10 Hz nominal in ms: a base bin is 64 intervals = 6.4 s. Stamps run
    // 0..20 s, then jump to 60..65 s: bins 4..=8 (25.6 s .. 57.6 s) are empty.
    let mut stamps: Vec<i64> = (0..200).map(|i| 1_000_000 + i * 100).collect();
    stamps.extend((0..50).map(|i| 1_000_000 + 60_000 + i * 100));
    let values: Vec<f32> = (0..250).map(|i| i as f32).collect();
    let mut stream = Stream::timestamped("s", ClockDomain::new("custom", "ms"), 10.0);
    stream.channels.push(Channel::new("v", Dtype::F32, ""));
    let mut header = Header::new("holes");
    header.streams.push(stream);
    let rec = Recording::create(
        tmp.path().join("holes.sigrec"),
        header,
        &[StreamData::timestamped(stamps, vec![Column::F32(values)])],
    )
    .unwrap();

    let (pyramid, _) = rec.pyramid("s", "v").unwrap();
    assert_eq!(pyramid.base_bin_s(), 6.4);
    let base = pyramid.level(0);
    assert_eq!(base.len(), 11, "stamps up to 64.9 s span eleven 6.4 s bins");
    assert_eq!((base[0].min, base[0].max), (0.0, 63.0));
    assert_eq!((base[3].min, base[3].max), (192.0, 199.0));
    for (k, bin) in base.iter().enumerate().take(9).skip(4) {
        assert!(bin.is_empty(), "bin {k} covers the hole");
    }
    assert_eq!((base[9].min, base[9].max), (200.0, 239.0));
    assert_eq!((base[10].min, base[10].max), (240.0, 249.0));
}

#[test]
fn a_cache_claiming_absurd_sizes_is_rejected_without_trying_to_allocate_them() {
    let pyramid = Pyramid::build_fixed_rate(100.0, 1000, |i| i as f64);
    let fingerprint = sigkit_format::Fingerprint {
        files: vec![(4000, 123)],
        dtype: Dtype::F32,
        base_bin_s: 0.64,
    };
    let good = pyramid.to_cache_bytes(&fingerprint);
    assert_eq!(
        Pyramid::from_cache_bytes(&good, &fingerprint),
        Some(pyramid)
    );

    // magic 8, file count 4, one (len, mtime) 24, dtype 1, base bin 8: the level count.
    let mut levels = good.clone();
    levels[45..49].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Pyramid::from_cache_bytes(&levels, &fingerprint), None);
    // Then the first level's bin count.
    let mut bins = good.clone();
    bins[49..57].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(Pyramid::from_cache_bytes(&bins, &fingerprint), None);
    assert_eq!(
        Pyramid::from_cache_bytes(&good[..good.len() - 1], &fingerprint),
        None
    );
}

#[test]
fn a_time_column_whose_last_stamp_is_absurdly_far_fails_with_the_stream_named() {
    let tmp = tempfile::tempdir().unwrap();
    let mut stamps: Vec<i64> = (0..100).map(|i| i * 10_000_000).collect();
    stamps.push(i64::MAX / 2);
    let mut stream = Stream::timestamped("imu", ClockDomain::new("custom", "ns"), 100.0);
    stream.channels.push(Channel::new("v", Dtype::F32, ""));
    let mut header = Header::new("corrupt");
    header.streams.push(stream);
    let rec = Recording::create(
        tmp.path().join("corrupt.sigrec"),
        header,
        &[StreamData::timestamped(
            stamps,
            vec![Column::F32(vec![0.0; 101])],
        )],
    )
    .unwrap();
    let err = rec.pyramid("imu", "v").unwrap_err().to_string();
    assert!(err.contains("imu"), "{err}");
}
