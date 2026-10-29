//! Seam: the `sigkit` binary. `gen` writes the three presets with the
//! parameters it was given; `inspect` prints every stream and channel with
//! counts, duration, nominal and fitted rate.

use std::path::Path;
use std::process::Command;

fn sigkit(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sigkit"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn inspect(path: &Path) -> String {
    let (ok, stdout, stderr) = sigkit(&["inspect", path.to_str().unwrap()]);
    assert!(ok, "inspect failed: {stderr}");
    stdout
}

/// The number before " ppm)" on the line that reports the fitted rate.
fn fitted_ppm(report: &str) -> f64 {
    let line = report
        .lines()
        .find(|l| l.contains("fitted"))
        .expect("a fitted rate line");
    let end = line.find(" ppm)").expect("ppm in parentheses");
    let start = line[..end].rfind('(').unwrap() + 1;
    line[start..end].parse().unwrap()
}

#[test]
fn gen_fixed_rate_then_inspect_lists_the_stream_channel_count_duration_and_rate() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("sine.sigrec");
    let (ok, _, stderr) = sigkit(&[
        "gen",
        "fixed-rate",
        out.to_str().unwrap(),
        "--rate",
        "50",
        "--duration",
        "2",
    ]);
    assert!(ok, "{stderr}");
    assert!(out.join("header.json").is_file());
    assert!(out.join("signal.value.bin").is_file());

    let report = inspect(&out);
    for needle in [
        "sine",
        "signal",
        "fixed_rate",
        "100 samples",
        "2.000 s",
        "rate 50.000 Hz",
        "value",
        "f32",
    ] {
        assert!(report.contains(needle), "missing {needle:?} in:\n{report}");
    }
}

#[test]
fn gen_timestamped_then_inspect_reports_a_fitted_rate_that_recovers_the_ppm() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("drift.sigrec");
    let (ok, _, stderr) = sigkit(&[
        "gen",
        "timestamped",
        out.to_str().unwrap(),
        "--rate",
        "416",
        "--duration",
        "10",
        "--jitter-us",
        "50",
        "--drop",
        "0.01",
        "--ppm",
        "150",
    ]);
    assert!(ok, "{stderr}");
    assert!(out.join("signal.t.bin").is_file());

    let report = inspect(&out);
    for needle in [
        "drift",
        "timestamped",
        "samples",
        "dropped",
        "nominal 416.000 Hz",
        "fitted",
        "sigkit.synthetic",
        "ns",
        "utc anchor",
    ] {
        assert!(report.contains(needle), "missing {needle:?} in:\n{report}");
    }
    let ppm = fitted_ppm(&report);
    assert!((ppm - 150.0).abs() < 2.0, "fitted {ppm} ppm, injected 150");
}

#[test]
fn gen_timestamped_with_drops_has_fewer_samples_than_nominal() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("drops.sigrec");
    let (ok, _, stderr) = sigkit(&[
        "gen",
        "timestamped",
        out.to_str().unwrap(),
        "--rate",
        "100",
        "--duration",
        "10",
        "--drop",
        "0.1",
    ]);
    assert!(ok, "{stderr}");
    let report = inspect(&out);
    let samples: usize = report
        .lines()
        .find_map(|l| {
            l.split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[1] == "samples")
                .map(|w| w[0].parse().unwrap())
        })
        .expect("a sample count");
    assert!(samples < 1000 && samples > 800, "{samples} samples");
}

#[test]
fn gen_pair_writes_two_recordings_with_their_own_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("pair");
    let (ok, _, stderr) = sigkit(&[
        "gen",
        "pair",
        base.to_str().unwrap(),
        "--rate",
        "200",
        "--duration",
        "5",
        "--jitter-us",
        "0",
        "--drop",
        "0",
        "--ppm-a",
        "-50",
        "--ppm-b",
        "200",
        "--shift",
        "0.25",
    ]);
    assert!(ok, "{stderr}");
    let a = tmp.path().join("pair-a.sigrec");
    let b = tmp.path().join("pair-b.sigrec");
    assert!(a.join("header.json").is_file() && b.join("header.json").is_file());

    let ppm_a = fitted_ppm(&inspect(&a));
    let ppm_b = fitted_ppm(&inspect(&b));
    assert!((ppm_a + 50.0).abs() < 0.5, "a: {ppm_a} ppm");
    assert!((ppm_b - 200.0).abs() < 0.5, "b: {ppm_b} ppm");
}

#[test]
fn gen_refuses_to_overwrite_and_inspect_reports_a_missing_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("once.sigrec");
    let (ok, _, _) = sigkit(&["gen", "fixed-rate", out.to_str().unwrap()]);
    assert!(ok);
    let (ok, _, stderr) = sigkit(&["gen", "fixed-rate", out.to_str().unwrap()]);
    assert!(!ok);
    assert!(stderr.contains("already exists"), "{stderr}");

    let missing = tmp.path().join("nope.sigrec");
    let (ok, _, stderr) = sigkit(&["inspect", missing.to_str().unwrap()]);
    assert!(!ok);
    assert!(stderr.contains("nope.sigrec"), "{stderr}");
}
