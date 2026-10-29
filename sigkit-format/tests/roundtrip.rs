//! Seam: read → write → read of a recording reproduces the header exactly,
//! including fields the crate does not know, and the typed facts the viewer
//! relies on (clock domain, nominal rate, time correction, calibration, UTC
//! anchor) survive.

use serde_json::Value;
use sigkit_format::{Dtype, Recording, StreamKind};
use std::fs;
use std::path::Path;

/// The header from the design spec, plus one unknown field at each level the
/// crate must carry through untouched. Known optional fields are all present,
/// because a rewrite normalises them (see the test on a minimal header below).
const HEADER: &str = r#"{
  "sigrec": 1,
  "name": "pixel7-drive",
  "created": "2026-10-29T12:00:00Z",
  "source": { "device": "Pixel 7", "os": "Android 15", "app": "sigkit-logger 0.1" },
  "logger_extra": { "battery_pct": 71, "tags": ["drive", "night"] },
  "streams": [
    {
      "id": "accel",
      "kind": "timestamped",
      "time": { "domain": "android.elapsedRealtimeNanos", "unit": "ns", "resolution_ns": 1 },
      "rate_nominal_hz": 416.0,
      "time_correction": { "k": 1.000012, "shift_s": -0.25 },
      "utc_anchor": { "mono": 123456789012, "utc_ns": 1761739200000000000, "fix": "gps" },
      "sensor_name": "LSM6DSO",
      "channels": [
        { "id": "x", "dtype": "f32", "unit": "m/s^2", "gain": 1.0, "offset": 0.0, "axis_hint": "forward" },
        { "id": "y", "dtype": "f32", "unit": "m/s^2", "gain": 1.0, "offset": 0.0 },
        { "id": "z", "dtype": "f32", "unit": "m/s^2", "gain": 1.0, "offset": 0.0 }
      ],
      "notes": "mounted on dash"
    },
    {
      "id": "legacy",
      "kind": "fixed_rate",
      "rate_hz": 100.0,
      "time_correction": { "k": 1.0, "shift_s": 0.0 },
      "channels": [ { "id": "ch0", "dtype": "i16", "unit": "", "gain": 0.001, "offset": -2.5 } ],
      "notes": ""
    }
  ],
  "expressions": [ { "id": "norm", "expr": "sqrt(x*x+y*y+z*z)" } ]
}"#;

fn write_le<T: Copy>(path: &Path, values: &[T], to_bytes: impl Fn(T) -> Vec<u8>) {
    let bytes: Vec<u8> = values.iter().flat_map(|v| to_bytes(*v)).collect();
    fs::write(path, bytes).unwrap();
}

fn write_fixture(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("header.json"), HEADER).unwrap();
    write_le(
        &dir.join("accel.t.bin"),
        &[0i64, 2_403_846, 4_807_692, 7_211_538],
        |v| v.to_le_bytes().to_vec(),
    );
    for ch in ["x", "y", "z"] {
        write_le(
            &dir.join(format!("accel.{ch}.bin")),
            &[0.5f32, -1.25, 9.81, 0.0],
            |v| v.to_le_bytes().to_vec(),
        );
    }
    write_le(&dir.join("legacy.ch0.bin"), &[-7i16, 0, 1234], |v| {
        v.to_le_bytes().to_vec()
    });
}

#[test]
fn read_write_read_reproduces_header_exactly_including_unknown_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pixel7-drive.sigrec");
    write_fixture(&dir);

    let first = Recording::open(&dir).unwrap();
    first.write_header().unwrap();
    let second = Recording::open(&dir).unwrap();

    assert_eq!(first.header(), second.header());
    let original: Value = serde_json::from_str(HEADER).unwrap();
    let rewritten: Value =
        serde_json::from_slice(&fs::read(dir.join("header.json")).unwrap()).unwrap();
    assert_eq!(
        original, rewritten,
        "rewritten header.json must equal the original as JSON"
    );

    let header = second.header();
    assert_eq!(header.name, "pixel7-drive");
    let accel = header.stream("accel").unwrap();
    assert_eq!(accel.kind, StreamKind::Timestamped);
    let clock = accel.time.as_ref().unwrap();
    assert_eq!(
        (clock.domain.as_str(), clock.unit.as_str()),
        ("android.elapsedRealtimeNanos", "ns")
    );
    assert_eq!(accel.rate_nominal_hz, Some(416.0));
    assert_eq!(
        (accel.time_correction.k, accel.time_correction.shift_s),
        (1.000012, -0.25)
    );
    let anchor = accel.utc_anchor.as_ref().unwrap();
    assert_eq!(
        (anchor.mono, anchor.utc_ns),
        (123456789012, 1761739200000000000)
    );
    assert_eq!(accel.channels[0].dtype, Dtype::F32);
    assert_eq!(accel.channels[0].unit, "m/s^2");

    let legacy = header.stream("legacy").unwrap();
    assert_eq!(legacy.kind, StreamKind::FixedRate);
    assert_eq!(legacy.rate_hz, Some(100.0));
    assert_eq!(
        (legacy.channels[0].gain, legacy.channels[0].offset),
        (0.001, -2.5)
    );
    assert_eq!(legacy.channels[0].dtype, Dtype::I16);
}

#[test]
fn columns_read_back_with_their_dtypes_and_the_time_column() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pixel7-drive.sigrec");
    write_fixture(&dir);

    let rec = Recording::open(&dir).unwrap();
    assert_eq!(rec.sample_count("accel"), Some(4));
    assert_eq!(rec.sample_count("legacy"), Some(3));

    let accel = rec.read_stream("accel").unwrap();
    assert_eq!(
        accel.time.as_deref(),
        Some(&[0i64, 2_403_846, 4_807_692, 7_211_538][..])
    );
    assert_eq!(accel.channels.len(), 3);
    assert_eq!(accel.channels[2].dtype(), Dtype::F32);
    assert_eq!(accel.channels[2].get(2), Some(9.81f32 as f64));

    let legacy = rec.read_stream("legacy").unwrap();
    assert!(legacy.time.is_none());
    assert_eq!(legacy.channels[0].get(0), Some(-7.0));
    assert_eq!(legacy.channels[0].get(2), Some(1234.0));
    assert_eq!(legacy.channels[0].get(3), None);
}

fn built_header() -> sigkit_format::Header {
    use sigkit_format::{Channel, ClockDomain, Header, Stream, UtcAnchor};
    let mut header = Header::new("built");
    let mut accel = Stream::timestamped(
        "accel",
        ClockDomain::new("android.elapsedRealtimeNanos", "ns"),
        416.0,
    );
    accel.utc_anchor = Some(UtcAnchor::new(1_000, 1_761_739_200_000_000_000));
    accel.channels.push(Channel::new("x", Dtype::F32, "m/s^2"));
    accel.channels.push(Channel::new("y", Dtype::I16, "m/s^2"));
    let mut baro = Stream::fixed_rate("baro", 1.0);
    baro.channels.push(Channel::new("p", Dtype::F64, "hPa"));
    header.streams.push(accel);
    header.streams.push(baro);
    header
}

#[test]
fn create_writes_a_recording_that_opens_with_the_same_header_and_samples() {
    use sigkit_format::{Column, StreamData};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("built.sigrec");
    let streams = vec![
        StreamData::timestamped(
            vec![1_000, 3_403_846, 5_807_692],
            vec![
                Column::F32(vec![0.5, -1.25, 9.81]),
                Column::I16(vec![-7, 0, 1234]),
            ],
        ),
        StreamData::fixed_rate(vec![Column::F64(vec![1013.25, 1013.0])]),
    ];

    let created = Recording::create(&dir, built_header(), &streams).unwrap();
    assert_eq!(created.sample_count("accel"), Some(3));
    let reopened = Recording::open(&dir).unwrap();
    assert_eq!(reopened.header(), &built_header());
    assert_eq!(reopened.read_stream("accel").unwrap(), streams[0]);
    assert_eq!(reopened.read_stream("baro").unwrap(), streams[1]);
}

#[test]
fn create_refuses_an_existing_recording_and_data_that_does_not_match_the_header() {
    use sigkit_format::{Column, Error, StreamData};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("built.sigrec");
    let good = vec![
        StreamData::timestamped(
            vec![1, 2],
            vec![Column::F32(vec![0.0, 1.0]), Column::I16(vec![0, 1])],
        ),
        StreamData::fixed_rate(vec![Column::F64(vec![1.0])]),
    ];
    Recording::create(&dir, built_header(), &good).unwrap();
    assert!(matches!(
        Recording::create(&dir, built_header(), &good),
        Err(Error::AlreadyExists { .. })
    ));

    let other = tmp.path().join("other.sigrec");
    let wrong_dtype = vec![
        StreamData::timestamped(
            vec![1, 2],
            vec![Column::F32(vec![0.0, 1.0]), Column::F32(vec![0.0, 1.0])],
        ),
        StreamData::fixed_rate(vec![Column::F64(vec![1.0])]),
    ];
    let err = Recording::create(&other, built_header(), &wrong_dtype).unwrap_err();
    assert!(
        matches!(&err, Error::InvalidStream { stream, .. } if stream == "accel"),
        "{err:?}"
    );
    assert!(
        !other.exists(),
        "nothing is written when the data does not match"
    );

    let short_time = vec![
        StreamData::timestamped(
            vec![1],
            vec![Column::F32(vec![0.0, 1.0]), Column::I16(vec![0, 1])],
        ),
        StreamData::fixed_rate(vec![Column::F64(vec![1.0])]),
    ];
    let err = Recording::create(&other, built_header(), &short_time).unwrap_err();
    assert!(
        matches!(&err, Error::ColumnLengthMismatch { stream, .. } if stream == "accel"),
        "{err:?}"
    );
}

#[test]
fn a_minimal_header_opens_and_its_rewrite_spells_out_the_known_defaults() {
    use sigkit_format::TIME_COLUMN;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("minimal.sigrec");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("header.json"),
        r#"{ "sigrec": 1, "name": "minimal", "streams": [
            { "id": "s", "kind": "timestamped", "time": { "domain": "custom", "unit": "us" },
              "rate_nominal_hz": 10.0, "channels": [ { "id": "v", "dtype": "i32" } ] } ] }"#,
    )
    .unwrap();
    fs::write(dir.join(format!("s.{TIME_COLUMN}.bin")), [0u8; 8 * 2]).unwrap();
    fs::write(dir.join("s.v.bin"), [0u8; 4 * 2]).unwrap();

    let rec = Recording::open(&dir).unwrap();
    let stream = rec.header().stream("s").unwrap();
    assert_eq!(
        (stream.time_correction.k, stream.time_correction.shift_s),
        (1.0, 0.0)
    );
    assert_eq!(
        (stream.channels[0].gain, stream.channels[0].offset),
        (1.0, 0.0)
    );
    assert_eq!(stream.seconds_per_tick(), Some(1e-6));

    rec.write_header().unwrap();
    let rewritten: Value =
        serde_json::from_slice(&fs::read(dir.join("header.json")).unwrap()).unwrap();
    assert_eq!(
        rewritten["streams"][0]["time_correction"],
        serde_json::json!({ "k": 1.0, "shift_s": 0.0 })
    );
    assert_eq!(rewritten["streams"][0]["notes"], "");
    assert_eq!(rewritten["streams"][0]["channels"][0]["gain"], 1.0);
    assert_eq!(rewritten["expressions"], serde_json::json!([]));
    assert_eq!(Recording::open(&dir).unwrap().header(), rec.header());
}
