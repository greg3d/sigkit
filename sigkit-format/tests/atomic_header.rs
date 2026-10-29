//! Seam: the header rewrite goes through a temp file and a rename, so a
//! rewrite that cannot complete leaves the old header byte-for-byte intact,
//! and a completed one leaves nothing behind but the new header.

use sigkit_format::Recording;
use std::fs;
use std::path::Path;

const HEADER: &str = r#"{ "sigrec": 1, "name": "atomic", "streams": [
  { "id": "s", "kind": "fixed_rate", "rate_hz": 10.0, "channels": [ { "id": "v", "dtype": "i16" } ] } ] }
"#;

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("atomic.sigrec");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("header.json"), HEADER).unwrap();
    fs::write(dir.join("s.v.bin"), [0u8; 2 * 3]).unwrap();
    (tmp, dir)
}

#[test]
fn a_completed_rewrite_leaves_only_the_new_header_in_the_directory() {
    let (_tmp, dir) = fixture();
    let mut rec = Recording::open(&dir).unwrap();
    rec.header_mut().stream_mut("s").unwrap().channels[0].gain = 0.5;
    rec.write_header().unwrap();

    let names = names_in(&dir);
    assert_eq!(names, ["header.json", "s.v.bin"]);
    let reopened = Recording::open(&dir).unwrap();
    assert_eq!(reopened.header().stream("s").unwrap().channels[0].gain, 0.5);
}

#[cfg(unix)]
#[test]
fn a_rewrite_that_cannot_complete_leaves_the_old_header_intact() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, dir) = fixture();
    let mut rec = Recording::open(&dir).unwrap();
    rec.header_mut().stream_mut("s").unwrap().channels[0].gain = 0.5;

    // The directory refuses new files, so nothing can be staged beside the
    // header. The existing header.json itself stays writable: a naive
    // truncate-and-write would still go through here and could half-write it.
    let writable = fs::metadata(&dir).unwrap().permissions();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = rec.write_header();
    fs::set_permissions(&dir, writable).unwrap();

    assert!(result.is_err(), "rewrite must report the failure");
    assert_eq!(fs::read_to_string(dir.join("header.json")).unwrap(), HEADER);
    let names = names_in(&dir);
    assert_eq!(
        names,
        ["header.json", "s.v.bin"],
        "no temp file may be left behind"
    );
}
