//! Legacy raw files with no header: guess what they hold, then import one by
//! writing a `header.json` beside it that reads it in place as a fixed-rate
//! stream. The data is never copied and the file never written.

use crate::atomic::write_atomic;
use crate::column::{ByteOrder, Dtype, Interleave};
use crate::error::{Error, Result};
use crate::header::{Channel, Header, RawLayout, Stream};
use crate::mapped::MappedColumn;
use crate::recording::{HEADER_FILE, Recording};
use crate::wallclock::rfc3339_now;
use serde_json::{Map, json};
use std::fs;
use std::path::{Path, PathBuf};

/// How to read a raw file: what the import sheet asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawImport {
    pub dtype: Dtype,
    pub byte_order: ByteOrder,
    /// Bytes of the file's own header, skipped.
    pub offset_bytes: u64,
    /// Interleaved channels, one element of each per frame.
    pub channels: usize,
    pub rate_hz: f64,
    /// Calibration given to every channel.
    pub gain: f64,
    pub offset: f64,
}

impl RawImport {
    /// Bytes per frame: one element of every channel.
    pub fn frame_bytes(&self) -> usize {
        self.channels * self.dtype.size()
    }

    /// Whole frames in a file of `file_bytes`, and the bytes after the last
    /// one, which are left out.
    pub fn frames(&self, file_bytes: u64) -> (usize, u64) {
        self.lane(0).frames(file_bytes)
    }

    /// Where channel `j`'s elements sit.
    pub fn lane(&self, j: usize) -> Interleave {
        Interleave {
            offset_bytes: self.offset_bytes,
            frame_bytes: self.frame_bytes(),
            lane_bytes: j * self.dtype.size(),
            byte_order: self.byte_order,
        }
    }

    /// Map every channel of `file` read this way, for a preview.
    pub fn map(&self, file: &Path) -> std::io::Result<Vec<MappedColumn>> {
        (0..self.channels)
            .map(|j| MappedColumn::open_interleaved(file, self.dtype, self.lane(j)))
            .collect()
    }

    /// The header that reads `file_name` this way: one fixed-rate stream
    /// named after the file, of channels `ch0`, `ch1`, ….
    pub fn header(&self, file_name: &str) -> Header {
        let stem = Path::new(file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.to_string());
        let mut stream = Stream::fixed_rate(stream_id(&stem), self.rate_hz);
        stream.raw = Some(RawLayout {
            file: file_name.to_string(),
            offset_bytes: self.offset_bytes,
            byte_order: self.byte_order,
            extra: Map::new(),
        });
        stream.channels = (0..self.channels)
            .map(|j| Channel {
                gain: self.gain,
                offset: self.offset,
                ..Channel::new(format!("ch{j}"), self.dtype, "")
            })
            .collect();
        let mut header = Header::new(stem);
        header.created = Some(rfc3339_now());
        header.source = Some(json!({
            "app": format!("sigkit import {}", env!("CARGO_PKG_VERSION")),
            "imported": file_name,
        }));
        header.streams.push(stream);
        header
    }
}

/// A stream id from a file stem: what ids may not hold becomes `_`.
fn stream_id(stem: &str) -> String {
    let id: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '.' | '/' | '\\') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if id.is_empty() { "raw".into() } else { id }
}

impl Recording {
    /// Import the raw file at `file`: write a `header.json` beside it that
    /// reads it as `import` says, and open the folder as a recording. Refuses
    /// a folder that already holds a header, and a layout the file cannot
    /// hold; in both cases nothing is written. The file itself is only read.
    pub fn import_raw(file: &Path, import: &RawImport) -> Result<Recording> {
        let io = |source| Error::Io {
            path: file.to_path_buf(),
            source,
        };
        let name = file
            .file_name()
            .ok_or_else(|| io(std::io::ErrorKind::InvalidInput.into()))?
            .to_string_lossy()
            .into_owned();
        let dir = match file.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let header_path = dir.join(HEADER_FILE);
        if header_path.exists() {
            return Err(Error::AlreadyExists { path: header_path });
        }
        let header = import.header(&name);
        header.validate()?;
        let stream = &header.streams[0];
        let bytes = fs::metadata(file).map_err(io)?.len();
        if bytes < import.offset_bytes || import.frames(bytes).0 == 0 {
            return Err(Error::InvalidStream {
                stream: stream.id.clone(),
                reason: format!(
                    "{bytes} bytes hold no whole {}-byte frame after a {}-byte header",
                    import.frame_bytes(),
                    import.offset_bytes
                ),
            });
        }
        let mut json = serde_json::to_vec_pretty(&header).map_err(|source| Error::Json {
            path: header_path.clone(),
            source,
        })?;
        json.push(b'\n');
        write_atomic(&header_path, &json).map_err(|source| Error::Io {
            path: header_path.clone(),
            source,
        })?;
        Recording::open(&dir).inspect_err(|_| {
            let _ = fs::remove_file(&header_path);
        })
    }
}

/// The recording `file` already belongs to: the folder whose `header.json`
/// reads it, as an imported raw file or as one of its own columns. `None`
/// for a file no header beside it mentions.
pub fn recording_of(file: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    let name = file.file_name()?.to_str()?;
    let header: Header = serde_json::from_slice(&fs::read(dir.join(HEADER_FILE)).ok()?).ok()?;
    let mentions = |stream: &Stream| {
        stream.raw.as_ref().is_some_and(|raw| raw.file == name)
            || stream.time.is_some() && stream.time_file_name() == name
            || stream
                .channels
                .iter()
                .any(|c| stream.channel_file_name(c) == name)
    };
    header
        .streams
        .iter()
        .any(mentions)
        .then(|| dir.to_path_buf())
}

/// What [`guess`] thinks a raw file holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guess {
    pub dtype: Dtype,
    pub byte_order: ByteOrder,
    pub channels: usize,
}

/// Most elements of a prefix [`guess`] decodes per candidate.
const GUESS_ELEMENTS: usize = 8192;
/// Most interleaved channels [`guess`] considers.
pub const GUESS_MAX_CHANNELS: usize = 8;

/// Guess a raw file's dtype, byte order and interleaved channel count from a
/// prefix of its samples, by decoding it as every dtype in both byte orders
/// and scoring how plausible each decoding is as a sampled signal:
///
/// - **bounded**: elements that decode to `NaN`, infinity, or a magnitude no
///   sensor writes (a float beyond 1e±12, an integer using the top bits of
///   its type) count against it, steeply;
/// - **not dominated by outliers**: elements far beyond the bulk count
///   against it ten times over;
/// - **smooth**: a sampled signal changes little from one sample of a channel
///   to the next compared with its spread, while bytes decoded at the wrong
///   width or order jump about. Roughness is measured at every lag up to
///   [`GUESS_MAX_CHANNELS`]; the lag where it is least is the channel count.
///
/// A wider dtype can look as good as the true one (an `i32` read from pairs of
/// `i16` is smooth too: its high half is the next sample), so among the
/// decodings scoring within a tenth of the best the narrowest wins, then the
/// best scoring, then little-endian. A prefix too short to judge guesses `f32`
/// little-endian, one channel.
pub fn guess(prefix: &[u8]) -> Guess {
    let mut scored: Vec<(Guess, f64)> = Vec::with_capacity(10);
    for dtype in Dtype::ALL {
        for byte_order in ByteOrder::ALL {
            let values: Vec<f64> = prefix
                .chunks_exact(dtype.size())
                .take(GUESS_ELEMENTS)
                .map(|b| dtype.decode(b, byte_order))
                .collect();
            if let Some((score, channels)) = plausibility(dtype, &values) {
                scored.push((
                    Guess {
                        dtype,
                        byte_order,
                        channels,
                    },
                    score,
                ));
            }
        }
    }
    let best = scored.iter().map(|(_, s)| *s).fold(0.0, f64::max);
    scored
        .into_iter()
        .filter(|(_, s)| *s >= 0.9 * best)
        .min_by(|(a, sa), (b, sb)| {
            a.dtype
                .size()
                .cmp(&b.dtype.size())
                .then(sb.total_cmp(sa))
                .then((a.byte_order == ByteOrder::Big).cmp(&(b.byte_order == ByteOrder::Big)))
        })
        .map(|(g, _)| g)
        .unwrap_or(Guess {
            dtype: Dtype::F32,
            byte_order: ByteOrder::Little,
            channels: 1,
        })
}

/// A decoding's plausibility in `(0, 1]` and the channel count it suggests;
/// `None` when there are too few elements to judge.
fn plausibility(dtype: Dtype, values: &[f64]) -> Option<(f64, usize)> {
    if values.len() < 4 * GUESS_MAX_CHANNELS {
        return None;
    }
    let bounded = |x: f64| match dtype {
        Dtype::F32 | Dtype::F64 => x == 0.0 || (x.is_finite() && (1e-12..1e12).contains(&x.abs())),
        Dtype::I16 => true,
        Dtype::I32 => x.abs() < (1u64 << 28) as f64,
        Dtype::I64 => x.abs() < (1u64 << 48) as f64,
    };
    let kept: Vec<Option<f64>> = values.iter().map(|&x| bounded(x).then_some(x)).collect();
    let good: Vec<f64> = kept.iter().flatten().copied().collect();
    let bounded_fraction = good.len() as f64 / values.len() as f64;
    if good.is_empty() {
        return Some((f64::MIN_POSITIVE, 1));
    }
    let mean = good.iter().sum::<f64>() / good.len() as f64;
    let spread = good.iter().map(|x| (x - mean).abs()).sum::<f64>() / good.len() as f64;
    // Roughness at each lag: mean step between samples `lag` apart over the
    // spread. A smooth signal is near 0, white noise near √2.
    let roughness = |lag: usize| {
        if spread == 0.0 {
            return 0.0;
        }
        let steps: Vec<f64> = kept
            .iter()
            .zip(&kept[lag..])
            .filter_map(|(a, b)| Some((b.as_ref()? - a.as_ref()?).abs()))
            .collect();
        if steps.is_empty() {
            return 2.0;
        }
        steps.iter().sum::<f64>() / steps.len() as f64 / spread
    };
    let lags: Vec<f64> = (1..=GUESS_MAX_CHANNELS).map(roughness).collect();
    let least = lags.iter().copied().fold(f64::INFINITY, f64::min);
    // The fewest channels whose roughness is near the least: one smooth
    // channel is smooth at every lag, only less so further apart.
    let channels = 1 + lags
        .iter()
        .position(|&r| r <= least * 1.25 + 1e-9)
        .expect("the least is among them");
    // Not dominated by outliers: a decoding where a few elements sit far
    // beyond the bulk (by fifty median deviations) is noise in a signal's
    // clothing.
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let centre = median(&mut good.clone());
    let deviation = median(&mut good.iter().map(|x| (x - centre).abs()).collect());
    let outliers = if deviation > 0.0 {
        good.iter()
            .filter(|x| (*x - centre).abs() > 50.0 * deviation)
            .count() as f64
            / good.len() as f64
    } else {
        0.0
    };
    let tame = 1.0 - (outliers * 10.0).min(1.0);
    Some((bounded_fraction.powi(4) * tame / (1.0 + least), channels))
}
