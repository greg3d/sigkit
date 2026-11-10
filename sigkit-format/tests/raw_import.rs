//! Seam: importing a legacy raw file. A `header.json` written beside the file
//! describes it as a fixed-rate stream read in place (dtype, byte order,
//! header offset, interleaved channels); the file then opens like any
//! recording and its bytes never change.

use sigkit_format::raw::{RawImport, recording_of};
use sigkit_format::synth::{self, RawSpec};
use sigkit_format::{ByteOrder, Dtype, Error, Recording, StreamKind};
use std::fs;
use std::path::{Path, PathBuf};

fn write_raw(dir: &Path, name: &str, spec: &RawSpec) -> (PathBuf, Vec<Vec<f64>>) {
    let raw = synth::raw(spec);
    let file = dir.join(name);
    fs::write(&file, &raw.bytes).unwrap();
    (file, raw.channels)
}

fn import_of(spec: &RawSpec) -> RawImport {
    RawImport {
        dtype: spec.dtype,
        byte_order: spec.byte_order,
        offset_bytes: spec.header_bytes as u64,
        channels: spec.channels,
        rate_hz: spec.rate_hz,
        gain: 1.0,
        offset: 0.0,
    }
}

#[test]
fn an_interleaved_big_endian_file_after_a_text_header_opens_as_a_fixed_rate_stream_of_its_channels()
{
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec {
        dtype: Dtype::I16,
        byte_order: ByteOrder::Big,
        channels: 2,
        header_bytes: 512,
        rate_hz: 250.0,
        duration_s: 4.0,
        ..RawSpec::default()
    };
    let (file, truth) = write_raw(tmp.path(), "bench-run.dat", &spec);
    let before = (
        fs::read(&file).unwrap(),
        fs::metadata(&file).unwrap().modified().unwrap(),
    );

    let import = RawImport {
        gain: 1.0 / 2048.0,
        offset: 0.5,
        ..import_of(&spec)
    };
    let rec = Recording::import_raw(&file, &import).unwrap();

    assert_eq!(rec.dir(), tmp.path());
    assert!(tmp.path().join("header.json").is_file());
    let reopened = Recording::open(tmp.path()).unwrap();
    let header = reopened.header();
    assert_eq!(header.name, "bench-run");
    assert_eq!(header.streams.len(), 1);
    let stream = &header.streams[0];
    assert_eq!(stream.kind, StreamKind::FixedRate);
    assert_eq!(stream.rate_hz, Some(250.0));
    let ids: Vec<&str> = stream.channels.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["ch0", "ch1"]);
    for channel in &stream.channels {
        assert_eq!(channel.dtype, Dtype::I16);
        assert_eq!((channel.gain, channel.offset), (1.0 / 2048.0, 0.5));
    }
    assert_eq!(reopened.sample_count(&stream.id), Some(1000));

    // Read whole, mapped, and through the pyramid: all the generator's values.
    let data = reopened.read_stream(&stream.id).unwrap();
    for (j, channel) in stream.channels.iter().enumerate() {
        let read: Vec<f64> = (0..1000)
            .map(|i| data.channels[j].get(i).unwrap())
            .collect();
        assert_eq!(read, truth[j], "{} read whole", channel.id);
        let mapped = reopened.map_column(&stream.id, &channel.id).unwrap();
        let mut out = Vec::new();
        mapped.read_into(0..1000, &mut out);
        assert_eq!(out, truth[j], "{} mapped", channel.id);
        let (pyramid, _) = reopened.pyramid(&stream.id, &channel.id).unwrap();
        let lo = truth[j].iter().copied().fold(f64::INFINITY, f64::min) as f32;
        let hi = truth[j].iter().copied().fold(f64::NEG_INFINITY, f64::max) as f32;
        assert_eq!(
            pyramid.whole_signal(),
            Some((lo, hi)),
            "{} pyramid",
            channel.id
        );
    }

    let after = (
        fs::read(&file).unwrap(),
        fs::metadata(&file).unwrap().modified().unwrap(),
    );
    assert!(before == after, "the raw file's bytes changed");
    assert_eq!(recording_of(&file).as_deref(), Some(tmp.path()));
}

#[test]
fn every_dtype_reads_back_in_both_byte_orders() {
    for dtype in Dtype::ALL {
        for byte_order in ByteOrder::ALL {
            let tmp = tempfile::tempdir().unwrap();
            let spec = RawSpec {
                dtype,
                byte_order,
                duration_s: 1.0,
                ..RawSpec::default()
            };
            let (file, truth) = write_raw(tmp.path(), "x.bin", &spec);
            let rec = Recording::import_raw(&file, &import_of(&spec)).unwrap();
            let stream = &rec.header().streams[0];
            let data = rec.read_stream(&stream.id).unwrap();
            let read: Vec<f64> = (0..100).map(|i| data.channels[0].get(i).unwrap()).collect();
            assert_eq!(read, truth[0], "{dtype} {byte_order:?}");
        }
    }
}

#[test]
fn a_trailing_partial_frame_is_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec {
        dtype: Dtype::F32,
        channels: 3,
        duration_s: 1.0,
        ..RawSpec::default()
    };
    let (file, _) = write_raw(tmp.path(), "x.bin", &spec);
    let mut bytes = fs::read(&file).unwrap();
    bytes.extend_from_slice(&[0xAB; 7]);
    fs::write(&file, &bytes).unwrap();

    let import = import_of(&spec);
    assert_eq!(import.frames(bytes.len() as u64), (100, 7));
    let rec = Recording::import_raw(&file, &import).unwrap();
    assert_eq!(rec.sample_count(&rec.header().streams[0].id), Some(100));
}

#[test]
fn import_refuses_a_folder_that_already_holds_a_header_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec::default();
    let (file, _) = write_raw(tmp.path(), "x.bin", &spec);
    fs::write(tmp.path().join("header.json"), "{}").unwrap();

    let err = Recording::import_raw(&file, &import_of(&spec)).unwrap_err();
    assert!(matches!(err, Error::AlreadyExists { .. }), "{err}");
    assert_eq!(
        fs::read_to_string(tmp.path().join("header.json")).unwrap(),
        "{}"
    );
    assert_eq!(recording_of(&file), None);
}

#[test]
fn import_refuses_a_header_offset_past_the_end_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec {
        duration_s: 0.1,
        ..RawSpec::default()
    };
    let (file, _) = write_raw(tmp.path(), "x.bin", &spec);
    let import = RawImport {
        offset_bytes: 1 << 20,
        ..import_of(&spec)
    };
    assert!(Recording::import_raw(&file, &import).is_err());
    assert!(!tmp.path().join("header.json").exists());
}

#[test]
fn reimporting_with_another_layout_does_not_serve_the_old_layouts_cached_pyramid() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec {
        dtype: Dtype::I16,
        duration_s: 2.0,
        ..RawSpec::default()
    };
    let (file, _) = write_raw(tmp.path(), "x.bin", &spec);
    let rec = Recording::import_raw(&file, &import_of(&spec)).unwrap();
    let stream = rec.header().streams[0].id.clone();
    let (first, _) = rec.pyramid(&stream, "ch0").unwrap();
    drop(rec);

    fs::remove_file(tmp.path().join("header.json")).unwrap();
    let swapped = RawImport {
        byte_order: ByteOrder::Big,
        ..import_of(&spec)
    };
    let rec = Recording::import_raw(&file, &swapped).unwrap();
    let (second, _) = rec.pyramid(&stream, "ch0").unwrap();
    assert_ne!(first.whole_signal(), second.whole_signal());
}

#[test]
fn a_column_dropped_from_inside_a_recording_belongs_to_that_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r.sigrec");
    synth::fixed_rate(&synth::FixedRateSpec {
        name: "r".into(),
        rate_hz: 100.0,
        duration_s: 1.0,
        signal: synth::Signal::default(),
        seed: 1,
    })
    .write(&dir)
    .unwrap();
    assert_eq!(
        recording_of(&dir.join("signal.value.bin")).as_deref(),
        Some(dir.as_path())
    );
    let loose = tmp.path().join("loose.bin");
    fs::write(&loose, [0u8; 64]).unwrap();
    assert_eq!(recording_of(&loose), None);
}

#[test]
fn a_single_little_endian_channel_with_trailing_bytes_maps_and_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = RawSpec {
        duration_s: 1.0,
        ..RawSpec::default()
    };
    let (file, truth) = write_raw(tmp.path(), "x.bin", &spec);
    let mut bytes = fs::read(&file).unwrap();
    bytes.extend_from_slice(&[0xAB; 3]);
    fs::write(&file, &bytes).unwrap();

    let import = import_of(&spec);
    let preview = import.map(&file).unwrap();
    assert_eq!(preview[0].len(), 100);
    let rec = Recording::import_raw(&file, &import).unwrap();
    let mapped = rec.map_column(&rec.header().streams[0].id, "ch0").unwrap();
    let mut out = Vec::new();
    mapped.read_into(0..100, &mut out);
    assert_eq!(out, truth[0]);
}
