//! Seam: a recording whose columns disagree in length fails to open with an
//! error naming the stream. Nothing is silently truncated.

use sigkit_format::{Error, Recording};
use std::fs;
use std::path::Path;

const HEADER: &str = r#"{
  "sigrec": 1,
  "name": "short",
  "streams": [
    { "id": "gyro", "kind": "timestamped", "time": { "domain": "ios.systemUptime", "unit": "ns" },
      "rate_nominal_hz": 100.0,
      "channels": [ { "id": "x", "dtype": "f32" }, { "id": "y", "dtype": "f32" } ] },
    { "id": "baro", "kind": "fixed_rate", "rate_hz": 1.0,
      "channels": [ { "id": "p", "dtype": "f64" } ] }
  ]
}"#;

fn write_fixture(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("header.json"), HEADER).unwrap();
    fs::write(dir.join("gyro.t.bin"), [0u8; 8 * 5]).unwrap();
    fs::write(dir.join("gyro.x.bin"), [0u8; 4 * 5]).unwrap();
    fs::write(dir.join("gyro.y.bin"), [0u8; 4 * 5]).unwrap();
    fs::write(dir.join("baro.p.bin"), [0u8; 8 * 3]).unwrap();
}

#[test]
fn a_short_channel_column_fails_to_open_naming_the_stream_and_the_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("short.sigrec");
    write_fixture(&dir);
    fs::write(dir.join("gyro.y.bin"), [0u8; 4 * 4]).unwrap();

    let err = Recording::open(&dir).unwrap_err();
    match &err {
        Error::ColumnLengthMismatch { stream, detail } => {
            assert_eq!(stream, "gyro");
            assert_eq!(detail, "t: 5, x: 5, y: 4");
        }
        other => panic!("expected ColumnLengthMismatch, got {other:?}"),
    }
    assert!(err.to_string().contains("gyro"), "{err}");
}

#[test]
fn a_short_time_column_is_a_mismatch_too() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("short.sigrec");
    write_fixture(&dir);
    fs::write(dir.join("gyro.t.bin"), [0u8; 8 * 4]).unwrap();

    let err = Recording::open(&dir).unwrap_err();
    assert!(
        matches!(&err, Error::ColumnLengthMismatch { stream, .. } if stream == "gyro"),
        "{err:?}"
    );
}

#[test]
fn a_column_file_that_is_not_a_whole_number_of_elements_is_an_error_naming_the_column() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("short.sigrec");
    write_fixture(&dir);
    fs::write(dir.join("baro.p.bin"), [0u8; 8 * 3 + 3]).unwrap();

    let err = Recording::open(&dir).unwrap_err();
    assert!(
        matches!(&err, Error::ColumnSize { stream, column, bytes: 27, .. } if stream == "baro" && column == "p"),
        "{err:?}"
    );
}

#[test]
fn a_missing_column_file_is_an_error_naming_the_stream_and_column() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("short.sigrec");
    write_fixture(&dir);
    fs::remove_file(dir.join("gyro.x.bin")).unwrap();

    let err = Recording::open(&dir).unwrap_err();
    assert!(
        matches!(&err, Error::MissingColumn { stream, column, .. } if stream == "gyro" && column == "x"),
        "{err:?}"
    );
}

#[test]
fn a_timestamped_stream_without_a_clock_domain_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bad.sigrec");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("header.json"),
        r#"{ "sigrec": 1, "name": "bad", "streams": [
            { "id": "a", "kind": "timestamped", "rate_nominal_hz": 10.0, "channels": [] } ] }"#,
    )
    .unwrap();

    let err = Recording::open(&dir).unwrap_err();
    assert!(
        matches!(&err, Error::InvalidStream { stream, .. } if stream == "a"),
        "{err:?}"
    );
}

fn header_with_ids(stream: &str, channel: &str) -> String {
    format!(
        r#"{{ "sigrec": 1, "name": "ids", "streams": [
            {{ "id": "{stream}", "kind": "fixed_rate", "rate_hz": 1.0, "channels": [ {{ "id": "{channel}", "dtype": "i16" }} ] }} ] }}"#
    )
}

#[test]
fn ids_may_be_any_text_that_can_name_a_column_file() {
    let tmp = tempfile::tempdir().unwrap();
    for (i, (stream, channel, ok)) in [
        ("gyrö", "ω-x", true),
        ("accel raw", "x", true),
        ("accel.raw", "x", false),
        ("accel", "x/y", false),
        ("accel", "t", false),
        ("", "x", false),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = tmp.path().join(format!("ids-{i}.sigrec"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("header.json"), header_with_ids(stream, channel)).unwrap();
        if ok {
            fs::write(dir.join(format!("{stream}.{channel}.bin")), [0u8; 2]).unwrap();
        }
        let result = Recording::open(&dir);
        assert_eq!(
            result.is_ok(),
            ok,
            "stream {stream:?} channel {channel:?}: {result:?}"
        );
        if !ok {
            assert!(matches!(result.unwrap_err(), Error::InvalidStream { .. }));
        }
    }
}
