//! `header.json`: the facts about a recording. The only file the viewer edits.
//!
//! Every object carries an `extra` map of fields this crate does not know, so a
//! header written by a newer logger survives a rewrite untouched. Known
//! optional fields (`time_correction`, `notes`, `expressions`, a channel's
//! `unit`, `gain` and `offset`) are normalised on rewrite: absent on read means
//! their default, and the rewrite writes that default out.

use crate::column::Dtype;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The `sigrec` header version this crate reads and writes.
pub const SIGREC_VERSION: u32 = 1;

/// Name of the time column in file names and error messages: `<stream>.t.bin`.
/// Reserved: no channel may use it.
pub const TIME_COLUMN: &str = "t";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub sigrec: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    pub streams: Vec<Stream>,
    /// Reserved for expression channels (v2). Carried through as-is.
    #[serde(default)]
    pub expressions: Vec<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Header {
    /// A version-1 header with no streams.
    pub fn new(name: impl Into<String>) -> Header {
        Header {
            sigrec: SIGREC_VERSION,
            name: name.into(),
            created: None,
            source: None,
            streams: Vec::new(),
            expressions: Vec::new(),
            extra: Map::new(),
        }
    }

    pub fn stream(&self, id: &str) -> Option<&Stream> {
        self.streams.iter().find(|s| s.id == id)
    }

    pub fn stream_mut(&mut self, id: &str) -> Option<&mut Stream> {
        self.streams.iter_mut().find(|s| s.id == id)
    }

    /// Check the facts the format relies on: version, stream ids, and that each
    /// stream kind carries what it needs.
    pub fn validate(&self) -> Result<()> {
        if self.sigrec != SIGREC_VERSION {
            return Err(Error::UnsupportedVersion(self.sigrec));
        }
        for (i, stream) in self.streams.iter().enumerate() {
            stream.validate()?;
            if self.streams[..i].iter().any(|other| other.id == stream.id) {
                return Err(Error::InvalidStream {
                    stream: stream.id.clone(),
                    reason: "duplicate stream id".into(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Per-sample time column; the stamp is the truth, any rate is derived.
    Timestamped,
    /// No time column; sample time is index divided by `rate_hz`.
    FixedRate,
}

impl StreamKind {
    pub fn name(self) -> &'static str {
        match self {
            StreamKind::Timestamped => "timestamped",
            StreamKind::FixedRate => "fixed_rate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    pub id: String,
    pub kind: StreamKind,
    /// Clock domain of the time column. Timestamped streams only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<ClockDomain>,
    /// The rate the logger requested or the sensor reported. Timestamped streams only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_nominal_hz: Option<f64>,
    /// The sample rate. Fixed-rate streams only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_hz: Option<f64>,
    #[serde(default)]
    pub time_correction: TimeCorrection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_anchor: Option<UtcAnchor>,
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub notes: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Stream {
    pub fn timestamped(id: impl Into<String>, time: ClockDomain, rate_nominal_hz: f64) -> Stream {
        Stream {
            id: id.into(),
            kind: StreamKind::Timestamped,
            time: Some(time),
            rate_nominal_hz: Some(rate_nominal_hz),
            rate_hz: None,
            time_correction: TimeCorrection::default(),
            utc_anchor: None,
            channels: Vec::new(),
            notes: String::new(),
            extra: Map::new(),
        }
    }

    pub fn fixed_rate(id: impl Into<String>, rate_hz: f64) -> Stream {
        Stream {
            id: id.into(),
            kind: StreamKind::FixedRate,
            time: None,
            rate_nominal_hz: None,
            rate_hz: Some(rate_hz),
            time_correction: TimeCorrection::default(),
            utc_anchor: None,
            channels: Vec::new(),
            notes: String::new(),
            extra: Map::new(),
        }
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// File name of the time column: `<stream>.t.bin`.
    pub fn time_file_name(&self) -> String {
        format!("{}.{TIME_COLUMN}.bin", self.id)
    }

    /// Seconds per tick of the time column; `None` for a fixed-rate stream or
    /// a clock unit this crate does not know.
    pub fn seconds_per_tick(&self) -> Option<f64> {
        self.time.as_ref()?.seconds_per_tick()
    }

    /// File name of a channel column: `<stream>.<channel>.bin`.
    pub fn channel_file_name(&self, channel: &Channel) -> String {
        format!("{}.{}.bin", self.id, channel.id)
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |reason: String| Error::InvalidStream {
            stream: self.id.clone(),
            reason,
        };
        if !is_valid_id(&self.id) {
            return Err(invalid(ID_RULE.into()));
        }
        match self.kind {
            StreamKind::Timestamped => {
                if self.time.is_none() {
                    return Err(invalid(
                        "timestamped stream needs a `time` clock domain".into(),
                    ));
                }
                if !self.rate_nominal_hz.is_some_and(positive_finite) {
                    return Err(invalid(
                        "timestamped stream needs a positive `rate_nominal_hz`".into(),
                    ));
                }
            }
            StreamKind::FixedRate => {
                if !self.rate_hz.is_some_and(positive_finite) {
                    return Err(invalid(
                        "fixed-rate stream needs a positive `rate_hz`".into(),
                    ));
                }
            }
        }
        for (i, channel) in self.channels.iter().enumerate() {
            if !is_valid_id(&channel.id) || channel.id == TIME_COLUMN {
                return Err(invalid(format!(
                    "channel id `{}`: {ID_RULE}, and `{TIME_COLUMN}` is reserved for the time column",
                    channel.id
                )));
            }
            if self.channels[..i]
                .iter()
                .any(|other| other.id == channel.id)
            {
                return Err(invalid(format!("duplicate channel id `{}`", channel.id)));
            }
        }
        Ok(())
    }
}

fn positive_finite(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

const ID_RULE: &str = "id must be non-empty and contain no `.`, `/`, `\\` or control characters";

/// Ids become file names (`<stream>.<channel>.bin`), so they must be
/// non-empty and free of the separator `.`, path separators and control
/// characters. Anything else, including non-ASCII, is fine.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id
            .chars()
            .any(|c| matches!(c, '.' | '/' | '\\') || c.is_control())
}

/// Which clock a time column counts in, and its unit (`ns`, `us`, `ms`, `s`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockDomain {
    pub domain: String,
    pub unit: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ClockDomain {
    pub fn new(domain: impl Into<String>, unit: impl Into<String>) -> ClockDomain {
        ClockDomain {
            domain: domain.into(),
            unit: unit.into(),
            extra: Map::new(),
        }
    }

    /// Seconds per tick of the time column, or `None` for a unit this crate does not know.
    pub fn seconds_per_tick(&self) -> Option<f64> {
        match self.unit.as_str() {
            "ns" => Some(1e-9),
            "us" => Some(1e-6),
            "ms" => Some(1e-3),
            "s" => Some(1.0),
            _ => None,
        }
    }
}

/// Per-stream time correction: display time is `k · t + shift_s`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeCorrection {
    pub k: f64,
    pub shift_s: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for TimeCorrection {
    fn default() -> Self {
        TimeCorrection {
            k: 1.0,
            shift_s: 0.0,
            extra: Map::new(),
        }
    }
}

/// A (monotonic stamp, UTC) pair that places the recording in wall-clock time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UtcAnchor {
    /// A stamp in the stream's clock domain.
    pub mono: i64,
    /// The UTC instant of that stamp, in nanoseconds since the Unix epoch.
    pub utc_ns: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl UtcAnchor {
    pub fn new(mono: i64, utc_ns: i64) -> UtcAnchor {
        UtcAnchor {
            mono,
            utc_ns,
            extra: Map::new(),
        }
    }
}

/// One scalar series inside a stream. Display value is `gain · raw + offset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub dtype: Dtype,
    #[serde(default)]
    pub unit: String,
    #[serde(default = "one")]
    pub gain: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> f64 {
    1.0
}

impl Channel {
    pub fn new(id: impl Into<String>, dtype: Dtype, unit: impl Into<String>) -> Channel {
        Channel {
            id: id.into(),
            dtype,
            unit: unit.into(),
            gain: 1.0,
            offset: 0.0,
            extra: Map::new(),
        }
    }
}
