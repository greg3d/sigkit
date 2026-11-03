//! A recording on disk: a `.sigrec` directory with `header.json` and planar columns.

use crate::atomic::write_atomic;
use crate::column::{Column, Dtype, time_from_le_bytes, time_to_le_bytes};
use crate::error::{Error, Result};
use crate::header::{Channel, ClockDomain, Header, Stream, StreamKind, TIME_COLUMN};
use crate::mapped::MappedColumn;
use crate::pyramid::{BASE_BIN_INTERVALS, Fingerprint, Pyramid};
use crate::ratefit::{RateFit, fit_rate_by};
use crate::residual::Residual;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const HEADER_FILE: &str = "header.json";
/// Regenerable, versioned derived data inside a recording: the pyramids.
pub const CACHE_DIR: &str = ".cache";

/// Where [`Recording::pyramid`] got its pyramid from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyramidSource {
    /// Loaded from `.cache/`, so the open was instant.
    Cache,
    /// Built from the column (first open, or the column changed) and cached.
    Built,
}

/// A stream's samples: the time column (timestamped streams only) and one
/// column per channel, in header order.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamData {
    pub time: Option<Vec<i64>>,
    pub channels: Vec<Column>,
}

impl StreamData {
    pub fn timestamped(time: Vec<i64>, channels: Vec<Column>) -> StreamData {
        StreamData {
            time: Some(time),
            channels,
        }
    }

    pub fn fixed_rate(channels: Vec<Column>) -> StreamData {
        StreamData {
            time: None,
            channels,
        }
    }
}

/// An open recording. Opening validates the header and every column length;
/// the raw column bytes are never rewritten through this type.
#[derive(Debug)]
pub struct Recording {
    dir: PathBuf,
    header: Header,
    sample_counts: HashMap<String, usize>,
}

impl Recording {
    /// Open an existing recording directory, validating the header and that
    /// every column of a stream holds the same number of samples.
    pub fn open(dir: impl AsRef<Path>) -> Result<Recording> {
        let dir = dir.as_ref().to_path_buf();
        let header_path = dir.join(HEADER_FILE);
        let bytes = fs::read(&header_path).map_err(|source| Error::Io {
            path: header_path.clone(),
            source,
        })?;
        let header: Header = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
            path: header_path,
            source,
        })?;
        header.validate()?;
        let sample_counts = header
            .streams
            .iter()
            .map(|stream| Ok((stream.id.clone(), validate_stream_columns(&dir, stream)?)))
            .collect::<Result<_>>()?;
        Ok(Recording {
            dir,
            header,
            sample_counts,
        })
    }

    /// Write a new recording: the directory, one column file per channel
    /// (and the time column of each timestamped stream), then the header.
    /// `streams` are in header order. Refuses a directory that already holds
    /// a header, and data whose shape disagrees with the header; in both
    /// cases nothing is written.
    pub fn create(
        dir: impl AsRef<Path>,
        header: Header,
        streams: &[StreamData],
    ) -> Result<Recording> {
        let dir = dir.as_ref().to_path_buf();
        let header_path = dir.join(HEADER_FILE);
        if header_path.exists() {
            return Err(Error::AlreadyExists { path: header_path });
        }
        header.validate()?;
        if streams.len() != header.streams.len() {
            return Err(Error::StreamCountMismatch {
                header: header.streams.len(),
                given: streams.len(),
            });
        }
        let sample_counts = header
            .streams
            .iter()
            .zip(streams)
            .map(|(stream, data)| Ok((stream.id.clone(), check_stream_data(stream, data)?)))
            .collect::<Result<HashMap<_, _>>>()?;

        fs::create_dir_all(&dir).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        for (stream, data) in header.streams.iter().zip(streams) {
            if let Some(time) = &data.time {
                write_file(&dir.join(stream.time_file_name()), &time_to_le_bytes(time))?;
            }
            for (channel, column) in stream.channels.iter().zip(&data.channels) {
                write_file(
                    &dir.join(stream.channel_file_name(channel)),
                    &column.to_le_bytes(),
                )?;
            }
        }
        let recording = Recording {
            dir,
            header,
            sample_counts,
        };
        recording.write_header()?;
        Ok(recording)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Edit header facts (calibration, corrections, notes). Persist with
    /// [`Recording::write_header`]. Column facts (channel lists, dtypes) are
    /// validated only at open.
    pub fn header_mut(&mut self) -> &mut Header {
        &mut self.header
    }

    /// Rewrite `header.json` atomically: a temp file beside it, fsync, rename.
    /// An interrupted write leaves the old header intact.
    pub fn write_header(&self) -> Result<()> {
        let path = self.dir.join(HEADER_FILE);
        let mut bytes = serde_json::to_vec_pretty(&self.header).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        bytes.push(b'\n');
        write_atomic(&path, &bytes).map_err(|source| Error::Io { path, source })
    }

    /// Samples per stream, as validated at open.
    pub fn sample_count(&self, stream_id: &str) -> Option<usize> {
        self.sample_counts.get(stream_id).copied()
    }

    /// Read a stream's time column; `None` for a fixed-rate stream.
    pub fn read_time(&self, stream_id: &str) -> Result<Option<Vec<i64>>> {
        let stream = self.stream(stream_id)?;
        self.read_time_of(stream)
    }

    /// Read a whole stream into memory.
    pub fn read_stream(&self, stream_id: &str) -> Result<StreamData> {
        let stream = self.stream(stream_id)?;
        let time = self.read_time_of(stream)?;
        let channels = stream
            .channels
            .iter()
            .map(|channel| {
                let path = self.dir.join(stream.channel_file_name(channel));
                let bytes = read_file(&path)?;
                Column::from_le_bytes(channel.dtype, &bytes).ok_or_else(|| {
                    column_size_error(
                        stream,
                        &channel.id,
                        channel.dtype,
                        &path,
                        bytes.len() as u64,
                    )
                })
            })
            .collect::<Result<_>>()?;
        Ok(StreamData { time, channels })
    }

    /// Map a channel's column read-only. Pages are loaded as they are touched.
    pub fn map_column(&self, stream_id: &str, channel_id: &str) -> Result<MappedColumn> {
        let stream = self.stream(stream_id)?;
        let channel = channel_of(stream, channel_id)?;
        let path = self.dir.join(stream.channel_file_name(channel));
        MappedColumn::open(&path, channel.dtype).map_err(|source| Error::Io { path, source })
    }

    /// Map a stream's time column read-only; `None` for a fixed-rate stream.
    pub fn map_time(&self, stream_id: &str) -> Result<Option<MappedColumn>> {
        let stream = self.stream(stream_id)?;
        if stream.kind != StreamKind::Timestamped {
            return Ok(None);
        }
        let path = self.dir.join(stream.time_file_name());
        MappedColumn::open(&path, Dtype::I64)
            .map(Some)
            .map_err(|source| Error::Io { path, source })
    }

    /// The channel's pyramid: from `.cache/` when it matches the columns on
    /// disk, otherwise built from the mapped column and cached. Writing the
    /// cache is best effort, so a read-only recording still opens.
    pub fn pyramid(&self, stream_id: &str, channel_id: &str) -> Result<(Pyramid, PyramidSource)> {
        self.pyramid_with_progress(stream_id, channel_id, &mut |_, _| {})
    }

    /// [`Recording::pyramid`], reporting `(samples done, samples total)` now
    /// and then while a pyramid is being built.
    pub fn pyramid_with_progress(
        &self,
        stream_id: &str,
        channel_id: &str,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(Pyramid, PyramidSource)> {
        let stream = self.stream(stream_id)?;
        let channel = channel_of(stream, channel_id)?;
        let rate = match stream.kind {
            StreamKind::FixedRate => stream.rate_hz,
            StreamKind::Timestamped => stream.rate_nominal_hz,
        }
        .expect("validated at open");
        let base_bin_s = BASE_BIN_INTERVALS as f64 / rate;

        let mut files = vec![file_stamp(
            &self.dir.join(stream.channel_file_name(channel)),
        )?];
        if stream.kind == StreamKind::Timestamped {
            files.push(file_stamp(&self.dir.join(stream.time_file_name()))?);
        }
        // Without a modification time a same-length rewrite would go
        // unnoticed, so such a column is never served from the cache.
        let files: Option<Vec<(u64, u128)>> = files.into_iter().collect();
        let fingerprint = files.map(|files| Fingerprint {
            files,
            dtype: channel.dtype,
            base_bin_s,
        });
        let cache_path = self
            .dir
            .join(CACHE_DIR)
            .join(format!("{}.{}.pyr", stream.id, channel.id));
        if let Some(fingerprint) = &fingerprint
            && let Ok(bytes) = fs::read(&cache_path)
            && let Some(pyramid) = Pyramid::from_cache_bytes(&bytes, fingerprint)
        {
            return Ok((pyramid, PyramidSource::Cache));
        }

        let column = self.map_column(stream_id, channel_id)?;
        column.advise_sequential();
        let n = column.len();
        let mut report = |i: usize| {
            if i.is_multiple_of(1 << 16) {
                progress(i, n);
            }
        };
        let value = |i: usize| column.get(i).expect("i < len");
        let pyramid = match self.map_time(stream_id)? {
            None => Pyramid::build_fixed_rate(rate, n, |i| {
                report(i);
                value(i)
            }),
            Some(time) => self.build_time_binned(stream, &time, &mut |i| {
                report(i);
                value(i)
            })?,
        };
        progress(n, n);
        column.advise_random();

        if let Some(fingerprint) = &fingerprint
            && fs::create_dir_all(self.dir.join(CACHE_DIR)).is_ok()
        {
            let _ = write_atomic(&cache_path, &pyramid.to_cache_bytes(fingerprint));
        }
        Ok((pyramid, PyramidSource::Built))
    }

    /// Seconds from the recording's zero to the stream's first sample. Each
    /// clock domain has its own zero: the earliest first stamp of the
    /// recording's streams in that domain, whatever their units. A fixed-rate
    /// stream starts at the zero.
    pub fn start_s(&self, stream_id: &str) -> Result<f64> {
        let stream = self.stream(stream_id)?;
        let Some(first) = self.first_stamp_ns(stream)? else {
            return Ok(0.0);
        };
        let domain = &stream.time.as_ref().expect("timestamped").domain;
        let mut zero = first;
        for other in &self.header.streams {
            if other.time.as_ref().is_some_and(|t| &t.domain == domain)
                && let Some(stamp) = self.first_stamp_ns(other)?
            {
                zero = zero.min(stamp);
            }
        }
        Ok((first - zero) as f64 * 1e-9)
    }

    /// The least-squares rate of a timestamped stream's stamps, read through
    /// the memory map; `None` for a fixed-rate stream, an unknown clock unit,
    /// or fewer than two stamps.
    pub fn fit_rate(&self, stream_id: &str) -> Result<Option<RateFit>> {
        let stream = self.stream(stream_id)?;
        let (Some(time), Some(seconds_per_tick), Some(nominal)) = (
            self.map_time(stream_id)?,
            stream.seconds_per_tick(),
            stream.rate_nominal_hz,
        ) else {
            return Ok(None);
        };
        time.advise_sequential();
        let fit = fit_rate_by(
            time.len(),
            |i| time.get_i64(i).expect("i < len"),
            seconds_per_tick,
            nominal,
        );
        time.advise_random();
        Ok(fit)
    }

    /// The pyramid of a timestamped stream's residual channel, binned by time
    /// exactly like its other channels. Built on every call, not cached: it
    /// depends on the residual's rate, and costs one pass over the time
    /// column.
    pub fn residual_pyramid(
        &self,
        stream_id: &str,
        residual: &Residual,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Pyramid> {
        let stream = self.stream(stream_id)?;
        let time = self
            .map_time(stream_id)?
            .ok_or_else(|| Error::InvalidStream {
                stream: stream.id.clone(),
                reason: "a fixed-rate stream has no residual".into(),
            })?;
        time.advise_sequential();
        let n = time.len();
        let pyramid = self.build_time_binned(stream, &time, &mut |i| {
            if i.is_multiple_of(1 << 16) {
                progress(i, n);
            }
            residual.at(i, time.get_i64(i).expect("i < len"))
        })?;
        progress(n, n);
        time.advise_random();
        Ok(pyramid)
    }

    /// Build a pyramid of `value(i)` for a timestamped stream: sample `i` goes
    /// to base bin `floor(Δticks · rate / ticks_per_second) / 64`, counted
    /// from the first stamp. The time column must not run backwards.
    fn build_time_binned(
        &self,
        stream: &Stream,
        time: &MappedColumn,
        value: &mut dyn FnMut(usize) -> f64,
    ) -> Result<Pyramid> {
        let invalid = |reason: String| Error::InvalidStream {
            stream: stream.id.clone(),
            reason,
        };
        let rate = stream.rate_nominal_hz.expect("validated at open");
        let ticks_per_second = stream
            .ticks_per_second()
            .ok_or_else(|| invalid("unknown clock unit, cannot bin by time".into()))?;
        let n = time.len();
        let zero = time.get_i64(0).unwrap_or(0);
        let interval = IntervalCounter::new(rate, ticks_per_second);
        let stamp = |i: usize| time.get_i64(i).expect("i < len");
        let bin_of = |stamp: i64| {
            usize::try_from(
                interval.whole_intervals(stamp.saturating_sub(zero)) / BASE_BIN_INTERVALS as i128,
            )
            .unwrap_or(usize::MAX)
        };
        let bins = if n == 0 {
            0
        } else {
            bin_of(stamp(n - 1)).saturating_add(1)
        };
        // A healthy stream spans about n / 64 base bins; holes add empty
        // ones. A last stamp that would need over a thousand times more bins
        // than samples is corrupt, not a hole.
        if bins > n.saturating_mul(1024).max(1 << 20) {
            return Err(invalid(format!(
                "time column spans {bins} base bins for {n} samples; a stamp looks corrupt"
            )));
        }
        let mut backwards = None;
        let mut previous = zero;
        let pyramid = Pyramid::build(
            BASE_BIN_INTERVALS as f64 / rate,
            bins,
            (0..n).map(|i| {
                let s = stamp(i);
                if s < previous && backwards.is_none() {
                    backwards = Some(i);
                }
                previous = s;
                (bin_of(s), value(i))
            }),
        );
        match backwards {
            Some(i) => Err(invalid(format!("time column runs backwards at sample {i}"))),
            None => Ok(pyramid),
        }
    }

    /// The first stamp of a timestamped stream in nanoseconds of its clock:
    /// exact for a unit of time, to the nearest nanosecond for a sample
    /// counter. `None` for a fixed-rate or empty stream, or an unknown unit.
    fn first_stamp_ns(&self, stream: &Stream) -> Result<Option<i128>> {
        let Some(ticks_per_second) = stream.ticks_per_second() else {
            return Ok(None);
        };
        let Some(first) = self.map_time(&stream.id)?.and_then(|time| time.get_i64(0)) else {
            return Ok(None);
        };
        Ok(Some(
            match stream.time.as_ref().and_then(ClockDomain::nanos_per_tick) {
                Some(nanos_per_tick) => first as i128 * nanos_per_tick as i128,
                None => (first as f64 * 1e9 / ticks_per_second).round() as i128,
            },
        ))
    }

    fn stream(&self, stream_id: &str) -> Result<&Stream> {
        self.header
            .stream(stream_id)
            .ok_or_else(|| Error::InvalidStream {
                stream: stream_id.to_string(),
                reason: "no such stream".into(),
            })
    }

    fn read_time_of(&self, stream: &Stream) -> Result<Option<Vec<i64>>> {
        match stream.kind {
            StreamKind::Timestamped => {
                let path = self.dir.join(stream.time_file_name());
                let bytes = read_file(&path)?;
                Ok(Some(time_from_le_bytes(&bytes).ok_or_else(|| {
                    column_size_error(stream, TIME_COLUMN, Dtype::I64, &path, bytes.len() as u64)
                })?))
            }
            StreamKind::FixedRate => Ok(None),
        }
    }
}

fn channel_of<'a>(stream: &'a Stream, channel_id: &str) -> Result<&'a Channel> {
    stream
        .channel(channel_id)
        .ok_or_else(|| Error::InvalidStream {
            stream: stream.id.clone(),
            reason: format!("no such channel `{channel_id}`"),
        })
}

/// `(length, mtime in ns since the Unix epoch)` of a column file; `None`
/// when the file system cannot report a modification time.
fn file_stamp(path: &Path) -> Result<Option<(u64, u128)>> {
    let meta = fs::metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (meta.len(), d.as_nanos())))
}

/// Counts whole nominal intervals in a span of clock ticks:
/// `floor(ticks · rate / ticks_per_second)`. Exact in integers when the rate
/// is a whole number of hertz (the usual case), so a stamp on a bin boundary
/// lands in the bin that starts there however long the recording; otherwise
/// in `f64`, exact while `ticks · rate` stays below 2^53.
struct IntervalCounter {
    rate: f64,
    ticks_per_second: f64,
    exact: Option<(i128, i128)>,
}

impl IntervalCounter {
    fn new(rate: f64, ticks_per_second: f64) -> IntervalCounter {
        let whole = |x: f64| (x.fract() == 0.0 && x > 0.0 && x < 1e15).then_some(x as i128);
        IntervalCounter {
            rate,
            ticks_per_second,
            exact: whole(rate).zip(whole(ticks_per_second)),
        }
    }

    fn whole_intervals(&self, ticks: i64) -> i128 {
        match self.exact {
            Some((rate, tps)) => (ticks as i128 * rate).div_euclid(tps).max(0),
            None => (ticks as f64 * self.rate / self.ticks_per_second)
                .floor()
                .max(0.0) as i128,
        }
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Check that `data` has the shape the header promises for `stream`: a time
/// column exactly when the stream is timestamped, one column per channel with
/// its dtype, and every column the same length. Returns that length.
fn check_stream_data(stream: &Stream, data: &StreamData) -> Result<usize> {
    let invalid = |reason: String| Error::InvalidStream {
        stream: stream.id.clone(),
        reason,
    };
    match (stream.kind, &data.time) {
        (StreamKind::Timestamped, None) => {
            return Err(invalid("timestamped stream needs a time column".into()));
        }
        (StreamKind::FixedRate, Some(_)) => {
            return Err(invalid(
                "fixed-rate stream must not have a time column".into(),
            ));
        }
        _ => {}
    }
    if data.channels.len() != stream.channels.len() {
        return Err(invalid(format!(
            "header lists {} channels but {} columns were given",
            stream.channels.len(),
            data.channels.len()
        )));
    }
    let mut lengths: Vec<(String, usize)> = Vec::with_capacity(stream.channels.len() + 1);
    if let Some(time) = &data.time {
        lengths.push((TIME_COLUMN.to_string(), time.len()));
    }
    for (channel, column) in stream.channels.iter().zip(&data.channels) {
        if column.dtype() != channel.dtype {
            return Err(invalid(format!(
                "channel `{}` is {} in the header but the column is {}",
                channel.id,
                channel.dtype,
                column.dtype()
            )));
        }
        lengths.push((channel.id.clone(), column.len()));
    }
    same_length(stream, &lengths)
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn column_size_error(
    stream: &Stream,
    column: &str,
    dtype: Dtype,
    path: &Path,
    bytes: u64,
) -> Error {
    Error::ColumnSize {
        stream: stream.id.clone(),
        column: column.to_string(),
        bytes,
        dtype: dtype.name(),
        path: path.to_path_buf(),
    }
}

/// Check that every column file of `stream` exists, has a whole number of
/// elements, and that all of them hold the same number of samples. Returns
/// that number.
fn validate_stream_columns(dir: &Path, stream: &Stream) -> Result<usize> {
    let mut lengths: Vec<(String, usize)> = Vec::with_capacity(stream.channels.len() + 1);
    if stream.kind == StreamKind::Timestamped {
        let file = stream.time_file_name();
        lengths.push((
            TIME_COLUMN.to_string(),
            column_length(dir, stream, TIME_COLUMN, &file, Dtype::I64)?,
        ));
    }
    for channel in &stream.channels {
        let file = stream.channel_file_name(channel);
        lengths.push((
            channel.id.clone(),
            column_length(dir, stream, &channel.id, &file, channel.dtype)?,
        ));
    }
    same_length(stream, &lengths)
}

/// All columns of a stream must hold the same number of samples. Returns it.
fn same_length(stream: &Stream, lengths: &[(String, usize)]) -> Result<usize> {
    let Some(&(_, first)) = lengths.first() else {
        return Ok(0);
    };
    if lengths.iter().any(|(_, n)| *n != first) {
        let detail = lengths
            .iter()
            .map(|(id, n)| format!("{id}: {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::ColumnLengthMismatch {
            stream: stream.id.clone(),
            detail,
        });
    }
    Ok(first)
}

fn column_length(
    dir: &Path,
    stream: &Stream,
    column: &str,
    file: &str,
    dtype: Dtype,
) -> Result<usize> {
    let elem_size = dtype.size() as u64;
    let path = dir.join(file);
    let meta = fs::metadata(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::MissingColumn {
                stream: stream.id.clone(),
                column: column.to_string(),
                path: path.clone(),
            }
        } else {
            Error::Io {
                path: path.clone(),
                source,
            }
        }
    })?;
    let bytes = meta.len();
    if bytes % elem_size != 0 {
        return Err(column_size_error(stream, column, dtype, &path, bytes));
    }
    Ok((bytes / elem_size) as usize)
}
