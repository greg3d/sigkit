//! A trace is one channel ready to draw: its values (a memory-mapped column
//! or a stream's residual), its clock (a rate or a memory-mapped time
//! column), its pyramid, its calibration and where its stream sits in display
//! time. It answers one question per frame: for these pixels along time,
//! what is the min and max in each?
//!
//! Three times appear here. A sample's *local* time is seconds from its
//! stream's first sample. The stream starts `start_s` after the recording's
//! zero. *Display* time applies the stream's time correction:
//! `k · (start_s + local) + shift_s`. Everything public speaks display time.

use crate::view::Span;
use sigkit_format::{Bin, Dtype, MappedColumn, Pyramid, Residual, UtcAnchor};

/// Tolerance, in pixels (or samples), when deciding which pixel a sample or
/// bin falls in: a value on a boundary belongs to the pixel that starts there
/// even when floating point puts it a hair below.
const EPS: f64 = 1e-6;

/// Two consecutive stamps further apart than this many nominal intervals
/// have a missing sample between them, and the line through raw samples
/// breaks there.
const BREAK_INTERVALS: f64 = 1.5;

/// How a viewport is drawn at a given time-per-pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// From this pyramid level: the coarsest whose bin fits in one pixel.
    Level(usize),
    /// Below the base bin: straight from the memory-mapped column.
    Raw,
}

/// Where a trace's samples come from.
pub enum Values {
    /// A channel's column.
    Column(MappedColumn),
    /// The residual of the trace's own time column, in seconds.
    Residual(Residual),
}

/// When a trace's samples were taken.
pub enum Clock {
    /// Sample `i` at `i / rate_hz`.
    Fixed { rate_hz: f64 },
    /// Sample `i` at its stamp, counted from the first.
    Stamped {
        time: MappedColumn,
        first: i64,
        seconds_per_tick: f64,
        nominal_hz: f64,
    },
}

impl Clock {
    /// A time column, read through its memory map.
    pub fn stamped(time: MappedColumn, seconds_per_tick: f64, nominal_hz: f64) -> Clock {
        Clock::Stamped {
            first: time.get_i64(0).unwrap_or(0),
            time,
            seconds_per_tick,
            nominal_hz,
        }
    }
}

/// Where a stream sits in display time: it starts `start_s` after the
/// recording's zero, and its time correction is `k · t + shift_s`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub start_s: f64,
    pub k: f64,
    pub shift_s: f64,
}

impl Placement {
    pub const ZERO: Placement = Placement {
        start_s: 0.0,
        k: 1.0,
        shift_s: 0.0,
    };

    fn display(&self, local: f64) -> f64 {
        self.k * (self.start_s + local) + self.shift_s
    }

    fn local(&self, display: f64) -> f64 {
        (display - self.shift_s) / self.k - self.start_s
    }
}

/// One sample as the cursor reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub index: usize,
    /// The sample's display instant.
    pub time: f64,
    /// `gain · raw + offset`.
    pub value: f64,
    /// The raw stamp, in the clock domain's unit, on a time column.
    pub stamp: Option<i64>,
    /// The column stores `f32`, so the value has only `f32` digits.
    pub single: bool,
}

impl Reading {
    /// The value with the digits its column holds and no more: an `f32`
    /// sample as the shortest text that reads back as that `f32`, anything
    /// wider to 15 significant digits. Very small or large values in
    /// exponent form.
    pub fn value_text(&self) -> String {
        fn text<T: std::fmt::Display + std::fmt::LowerExp>(v: T, magnitude: f64) -> String {
            if magnitude != 0.0 && !(1e-4..1e15).contains(&magnitude) {
                format!("{v:e}")
            } else {
                format!("{v}")
            }
        }
        let value = if self.value == 0.0 { 0.0 } else { self.value };
        if self.single {
            let v = value as f32;
            text(v, v.abs() as f64)
        } else {
            let v: f64 = format!("{value:.14e}").parse().unwrap_or(value);
            text(v, v.abs())
        }
    }
}

/// One channel ready to draw.
pub struct Trace {
    values: Values,
    clock: Clock,
    pyramid: Pyramid,
    gain: f64,
    offset: f64,
    placement: Placement,
    utc_anchor: Option<UtcAnchor>,
}

impl Trace {
    /// Uncalibrated, at the recording's zero, uncorrected.
    pub fn new(values: Values, clock: Clock, pyramid: Pyramid) -> Trace {
        Trace {
            values,
            clock,
            pyramid,
            gain: 1.0,
            offset: 0.0,
            placement: Placement::ZERO,
            utc_anchor: None,
        }
    }

    /// Display value is `gain · raw + offset`.
    pub fn calibrated(self, gain: f64, offset: f64) -> Trace {
        Trace {
            gain,
            offset,
            ..self
        }
    }

    pub fn placed(self, placement: Placement) -> Trace {
        Trace { placement, ..self }
    }

    /// The stream's UTC anchor, for the cursor readout. Meaningful only with
    /// a time column, whose clock the anchor's stamp counts in.
    pub fn anchored(self, utc_anchor: Option<UtcAnchor>) -> Trace {
        Trace { utc_anchor, ..self }
    }

    #[cfg(test)]
    pub fn pyramid(&self) -> &Pyramid {
        &self.pyramid
    }

    pub fn len(&self) -> usize {
        match &self.clock {
            Clock::Fixed { .. } => match &self.values {
                Values::Column(column) => column.len(),
                Values::Residual(_) => 0,
            },
            Clock::Stamped { time, .. } => time.len(),
        }
    }

    /// The fixed rate, or the nominal rate of a time column.
    pub fn rate_hz(&self) -> f64 {
        match self.clock {
            Clock::Fixed { rate_hz } => rate_hz,
            Clock::Stamped { nominal_hz, .. } => nominal_hz,
        }
    }

    /// From the first sample to one interval past the last, in display time;
    /// `None` when there is no sample.
    pub fn display_span(&self) -> Option<Span> {
        let last = self.len().checked_sub(1)?;
        let end = self.local_time(last) + 1.0 / self.rate_hz();
        Some(Span::new(
            self.placement.display(0.0),
            self.placement.display(end),
        ))
    }

    /// Min and max of the whole signal in display units; `None` when empty.
    pub fn whole_signal(&self) -> Option<(f64, f64)> {
        self.pyramid
            .whole_signal()
            .map(|(lo, hi)| self.calibrated_pair(lo as f64, hi as f64))
    }

    /// Raw samples one pixel covers at this display time-per-pixel.
    pub fn samples_per_px(&self, time_per_px: f64) -> f64 {
        time_per_px / self.placement.k * self.rate_hz()
    }

    /// True when, at this display time-per-pixel, raw samples are drawn as a
    /// line through their instants rather than as candles. A time column
    /// switches earlier, at two samples per pixel, so jitter never leaves a
    /// pixel without a sample and a gap is drawn only where one is.
    pub fn draws_samples(&self, time_per_px: f64) -> bool {
        let below = match self.clock {
            Clock::Fixed { .. } => 1.0,
            Clock::Stamped { .. } => 2.0,
        };
        self.samples_per_px(time_per_px) < below
    }

    /// Which level draws at this display time-per-pixel.
    pub fn detail(&self, time_per_px: f64) -> Detail {
        let time_per_px = time_per_px / self.placement.k;
        if time_per_px < self.pyramid.base_bin_s() {
            return Detail::Raw;
        }
        let mut level = 0;
        while level + 1 < self.pyramid.levels() && self.pyramid.bin_s(level + 1) <= time_per_px {
            level += 1;
        }
        Detail::Level(level)
    }

    /// One candle per pixel for `px` pixels covering display `time`, in
    /// display units. A pixel covering no sample is an empty bin: a gap.
    /// Above the base every bin goes to the pixel holding its centre, so
    /// every sample is drawn exactly once; below it, raw samples go to the
    /// pixel holding their instant, found by binary search on a time column.
    pub fn candles(&self, time: Span, px: usize, out: &mut Vec<Bin>) {
        out.clear();
        out.resize(px, Bin::EMPTY);
        if px == 0 || !time.len().is_finite() || time.len() <= 0.0 {
            return;
        }
        let detail = self.detail(time.len() / px as f64);
        let time = self.local_span(time);
        let time_per_px = time.len() / px as f64;
        let pixel_of = |t: f64| -> Option<usize> {
            let p = ((t - time.from) / time_per_px + EPS).floor();
            (p >= 0.0 && p < px as f64).then_some(p as usize)
        };
        match detail {
            Detail::Level(level) => {
                let bins = self.pyramid.level(level);
                let bin_s = self.pyramid.bin_s(level);
                let first = (time.from / bin_s - 0.5).ceil().max(0.0) as usize;
                let last = (time.to / bin_s - 0.5).floor();
                if last < 0.0 || first >= bins.len() {
                    return;
                }
                let last = (last as usize).min(bins.len() - 1);
                for (k, bin) in bins.iter().enumerate().take(last + 1).skip(first) {
                    if let Some(p) = pixel_of((k as f64 + 0.5) * bin_s) {
                        out[p] = out[p].merge(*bin);
                    }
                }
            }
            Detail::Raw => {
                for i in self.index_at(time.from)..self.index_at(time.to) {
                    if let Some(p) = pixel_of(self.local_time(i)) {
                        out[p].include(self.value(i));
                    }
                }
            }
        }
        for bin in out.iter_mut() {
            *bin = self.calibrated_bin(*bin);
        }
    }

    /// `(display instant, display value)` of every sample in display `time`,
    /// plus one neighbour on each side so a line through them enters and
    /// leaves the window. Where a time column is missing a sample, a `NaN`
    /// value between the two stamps breaks the line. For when a pixel covers
    /// less than a sample or two.
    pub fn samples(&self, time: Span, out: &mut Vec<(f64, f64)>) {
        out.clear();
        let n = self.len();
        if n == 0 {
            return;
        }
        let time = self.local_span(time);
        let first = self.index_at(time.from).saturating_sub(1);
        let last = self.index_after(time.to).min(n - 1);
        let gap = BREAK_INTERVALS / self.rate_hz();
        let mut previous: Option<f64> = None;
        for i in first..=last {
            let t = self.local_time(i);
            if let Some(p) = previous
                && t - p > gap
            {
                out.push((self.placement.display(0.5 * (p + t)), f64::NAN));
            }
            previous = Some(t);
            out.push((
                self.placement.display(t),
                self.gain * self.value(i) + self.offset,
            ));
        }
    }

    /// The exact min and max, in display units, of the samples whose instant
    /// lies in display `[time.from, time.to)`; `None` when there is none.
    /// Base bins wholly inside the window come from the pyramid, coarsest
    /// first; the samples in partial bins at the edges are read raw. The
    /// cost is logarithmic in the window.
    pub fn range(&self, time: Span) -> Option<(f64, f64)> {
        let time = self.local_span(time);
        let (first, end) = (self.index_at(time.from), self.index_at(time.to));
        if first >= end {
            return None;
        }
        let mut all = Bin::EMPTY;
        let mut raw = |from: usize, to: usize| {
            for i in from.max(first)..to.min(end) {
                all.include(self.value(i));
            }
        };
        let bin_s = self.pyramid.base_bin_s();
        let bins = self.pyramid.level(0).len();
        let mut a = ((time.from / bin_s - EPS).ceil().max(0.0) as usize).min(bins);
        let mut b = ((time.to / bin_s + EPS).floor().max(0.0) as usize).min(bins);
        if a >= b {
            raw(first, end);
        } else {
            // One sample either way of each bin edge: binning by integer
            // stamps and searching by float seconds may disagree on a stamp
            // that sits exactly on the edge, and min/max forgive repeats.
            raw(first, self.index_at(a as f64 * bin_s) + 1);
            raw(self.index_at(b as f64 * bin_s).saturating_sub(1), end);
            let mut level = 0;
            while a < b {
                let level_bins = self.pyramid.level(level);
                if a % 2 == 1 {
                    all = all.merge(level_bins[a]);
                    a += 1;
                }
                if b % 2 == 1 {
                    b -= 1;
                    all = all.merge(level_bins[b]);
                }
                (a, b, level) = (a / 2, b / 2, level + 1);
            }
        }
        (!all.is_empty()).then(|| self.calibrated_pair(all.min as f64, all.max as f64))
    }

    /// The sample nearest display time `t`: on a time column the one with
    /// the nearest stamp, never an interpolation; inside a hole, the nearer
    /// sample either side of it. `None` before the first sample or after the
    /// last by more than an interval.
    pub fn nearest(&self, t: f64) -> Option<Reading> {
        let n = self.len();
        let local = self.placement.local(t);
        let after = self.index_at(local);
        let index = match (after.checked_sub(1), after < n) {
            (Some(before), true) => {
                if local - self.local_time(before) <= self.local_time(after) - local {
                    before
                } else {
                    after
                }
            }
            (Some(before), false) => before,
            (None, true) => after,
            (None, false) => return None,
        };
        let at = self.local_time(index);
        let outside = after == 0 || after == n;
        if outside && (local - at).abs() > (1.0 + EPS) / self.rate_hz() {
            return None;
        }
        let stamp = match &self.clock {
            Clock::Stamped { time, .. } => time.get_i64(index),
            Clock::Fixed { .. } => None,
        };
        let single = matches!(&self.values, Values::Column(c) if c.dtype() == Dtype::F32);
        Some(Reading {
            index,
            time: self.placement.display(at),
            value: self.gain * self.value(index) + self.offset,
            stamp,
            single,
        })
    }

    /// The UTC instant, in ns since the Unix epoch, at display time `t` on
    /// this stream's clock; `None` without a time column and an anchor.
    pub fn utc_ns_at(&self, t: f64) -> Option<i64> {
        let anchor = self.utc_anchor.as_ref()?;
        let Clock::Stamped {
            first,
            seconds_per_tick,
            ..
        } = &self.clock
        else {
            return None;
        };
        let since_anchor_s =
            (first - anchor.mono) as f64 * seconds_per_tick + self.placement.local(t);
        Some(anchor.utc_ns + (since_anchor_s * 1e9).round() as i64)
    }

    fn local_span(&self, time: Span) -> Span {
        Span::new(
            self.placement.local(time.from),
            self.placement.local(time.to),
        )
    }

    /// Seconds from the first sample to sample `i`.
    fn local_time(&self, i: usize) -> f64 {
        match &self.clock {
            Clock::Fixed { rate_hz } => i as f64 / rate_hz,
            Clock::Stamped {
                time,
                first,
                seconds_per_tick,
                ..
            } => (time.get_i64(i).expect("i < len") - first) as f64 * seconds_per_tick,
        }
    }

    /// The raw value of sample `i`.
    fn value(&self, i: usize) -> f64 {
        match &self.values {
            Values::Column(column) => column.get(i).expect("i < len"),
            Values::Residual(residual) => match &self.clock {
                Clock::Stamped { time, .. } => residual.at(i, time.get_i64(i).expect("i < len")),
                Clock::Fixed { .. } => unreachable!("a residual has a time column"),
            },
        }
    }

    /// The first sample at or after local `t`, or `len` when none is.
    fn index_at(&self, t: f64) -> usize {
        self.partition(t, false)
    }

    /// The first sample after local `t`, or `len` when none is.
    fn index_after(&self, t: f64) -> usize {
        self.partition(t, true)
    }

    /// The first sample at or after (`after`: strictly after) local `t`,
    /// within `EPS` of an interval: arithmetic at a fixed rate, a binary
    /// search over a time column, which never runs backwards.
    fn partition(&self, t: f64, after: bool) -> usize {
        let n = self.len();
        let in_intervals = t * self.rate_hz();
        if let Clock::Fixed { .. } = self.clock {
            let i = if after {
                (in_intervals + EPS).floor() + 1.0
            } else {
                (in_intervals - EPS).ceil()
            };
            return (i.max(0.0) as usize).min(n);
        }
        let before = |i: usize| {
            let at = self.local_time(i) * self.rate_hz();
            if after {
                at <= in_intervals + EPS
            } else {
                at < in_intervals - EPS
            }
        };
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if before(mid) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    fn calibrated_bin(&self, bin: Bin) -> Bin {
        if bin.is_empty() {
            return Bin::EMPTY;
        }
        let (min, max) = self.calibrated_pair(bin.min as f64, bin.max as f64);
        Bin {
            min: min as f32,
            max: max as f32,
        }
    }

    /// `gain · raw + offset` on both ends; a negative gain swaps them.
    fn calibrated_pair(&self, lo: f64, hi: f64) -> (f64, f64) {
        let a = self.gain * lo + self.offset;
        let b = self.gain * hi + self.offset;
        (a.min(b), a.max(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sigkit_format::Channel;
    use sigkit_format::{ClockDomain, Column, Header, Recording, Stream, StreamData};

    /// 1000 samples at 100 Hz, sample `i` holds the value `i`.
    fn ramp(dir: &std::path::Path, gain: f64, offset: f64) -> Trace {
        let mut stream = Stream::fixed_rate("s", 100.0);
        stream.channels.push(Channel::new("v", Dtype::F32, ""));
        let mut header = Header::new("ramp");
        header.streams.push(stream);
        let values: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let rec = Recording::create(
            dir,
            header,
            &[StreamData::fixed_rate(vec![Column::F32(values)])],
        )
        .unwrap();
        let column = rec.map_column("s", "v").unwrap();
        let (pyramid, _) = rec.pyramid("s", "v").unwrap();
        Trace::new(
            Values::Column(column),
            Clock::Fixed { rate_hz: 100.0 },
            pyramid,
        )
        .calibrated(gain, offset)
    }

    /// A 100 Hz timestamped stream in ns, sample `i` holding the value `i`,
    /// stamped by `stamp(i)` and starting at 7 s of uptime.
    fn stamped(dir: &std::path::Path, n: usize, stamp: impl Fn(usize) -> i64) -> (Trace, Vec<i64>) {
        let mut stream = Stream::timestamped("s", ClockDomain::new("test", "ns"), 100.0);
        stream.channels.push(Channel::new("v", Dtype::F32, ""));
        let mut header = Header::new("stamped");
        header.streams.push(stream);
        let stamps: Vec<i64> = (0..n).map(|i| 7_000_000_000 + stamp(i)).collect();
        let values: Vec<f32> = (0..n).map(|i| i as f32).collect();
        let rec = Recording::create(
            dir,
            header,
            &[StreamData::timestamped(
                stamps.clone(),
                vec![Column::F32(values)],
            )],
        )
        .unwrap();
        let (pyramid, _) = rec.pyramid("s", "v").unwrap();
        let trace = Trace::new(
            Values::Column(rec.map_column("s", "v").unwrap()),
            Clock::stamped(rec.map_time("s").unwrap().unwrap(), 1e-9, 100.0),
            pyramid,
        );
        (trace, stamps)
    }

    /// 10 ms apart with a deterministic jitter of up to ±2 ms.
    fn jittered(i: usize) -> i64 {
        let jitter = ((i * 7919) % 401) as i64 - 200;
        i as i64 * 10_000_000 + jitter * 10_000
    }

    fn pairs(out: &[Bin]) -> Vec<(f32, f32)> {
        out.iter().map(|b| (b.min, b.max)).collect()
    }

    #[test]
    fn the_coarsest_level_whose_bin_fits_in_a_pixel_is_drawn_and_raw_below_the_base() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        // Base bin is 0.64 s; levels are 0.64, 1.28, 2.56, 5.12 s.
        assert_eq!(trace.detail(0.64), Detail::Level(0));
        assert_eq!(trace.detail(1.0), Detail::Level(0));
        assert_eq!(trace.detail(1.28), Detail::Level(1));
        assert_eq!(trace.detail(3.0), Detail::Level(2));
        assert_eq!(
            trace.detail(1000.0),
            Detail::Level(trace.pyramid().levels() - 1)
        );
        assert_eq!(trace.detail(0.5), Detail::Raw);
    }

    #[test]
    fn above_the_base_every_sample_is_drawn_in_exactly_one_pixel_the_one_holding_its_bins_centre() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        let mut out = Vec::new();
        trace.candles(Span::new(0.0, 10.0), 10, &mut out);
        assert_eq!(
            pairs(&out),
            vec![
                (0.0, 127.0),
                (128.0, 191.0),
                (192.0, 319.0),
                (320.0, 383.0),
                (384.0, 511.0),
                (512.0, 575.0),
                (576.0, 703.0),
                (704.0, 767.0),
                (768.0, 895.0),
                (896.0, 999.0),
            ]
        );
    }

    #[test]
    fn below_the_base_each_pixel_holds_the_min_and_max_of_the_raw_samples_it_covers() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        let mut out = Vec::new();
        trace.candles(Span::new(0.0, 1.0), 10, &mut out);
        let expected: Vec<(f32, f32)> = (0..10)
            .map(|p| ((10 * p) as f32, (10 * p + 9) as f32))
            .collect();
        assert_eq!(pairs(&out), expected);
    }

    #[test]
    fn pixels_outside_the_signal_are_gaps() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        let mut out = Vec::new();
        trace.candles(Span::new(-1.0, 1.0), 4, &mut out);
        assert!(out[0].is_empty() && out[1].is_empty());
        assert_eq!(pairs(&out[2..]), vec![(0.0, 49.0), (50.0, 99.0)]);
        trace.candles(Span::new(9.0, 11.0), 4, &mut out);
        assert_eq!(pairs(&out[..2]), vec![(900.0, 949.0), (950.0, 999.0)]);
        assert!(out[2].is_empty() && out[3].is_empty());
    }

    #[test]
    fn samples_for_a_window_include_one_neighbour_on_each_side_so_the_line_enters_and_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        let mut out = Vec::new();
        trace.samples(Span::new(0.0, 0.05), &mut out);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0], (0.0, 0.0));
        assert_eq!(out[6], (0.06, 6.0));
        trace.samples(Span::new(0.105, 0.125), &mut out);
        assert_eq!(
            out,
            vec![(0.1, 10.0), (0.11, 11.0), (0.12, 12.0), (0.13, 13.0)]
        );
        assert_eq!(trace.samples_per_px(0.005), 0.5);
    }

    #[test]
    fn calibration_is_applied_to_what_is_drawn_and_a_negative_gain_swaps_min_and_max() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), -2.0, 1.0);
        let mut out = Vec::new();
        trace.candles(Span::new(0.0, 1.0), 1, &mut out);
        assert_eq!(pairs(&out), vec![(-253.0, 1.0)]);
        assert_eq!(trace.whole_signal(), Some((-1997.0, 1.0)));
        assert_eq!(trace.display_span(), Some(Span::new(0.0, 10.0)));
        let mut samples = Vec::new();
        trace.samples(Span::new(0.0, 0.0), &mut samples);
        assert_eq!(samples[1], (0.01, -1.0));
    }

    #[test]
    fn the_range_of_a_window_is_exact_even_where_a_bin_straddles_its_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), 1.0, 0.0);
        // Samples with instants in [from, to): base bins are 64 samples, so
        // these windows cut through bins at both ends.
        assert_eq!(trace.range(Span::new(0.0, 5.0)), Some((0.0, 499.0)));
        assert_eq!(trace.range(Span::new(1.234, 7.89)), Some((124.0, 788.0)));
        assert_eq!(trace.range(Span::new(0.005, 0.015)), Some((1.0, 1.0)));
        assert_eq!(trace.range(Span::new(-5.0, 100.0)), Some((0.0, 999.0)));
        assert_eq!(trace.range(Span::new(10.0, 20.0)), None);
        assert_eq!(trace.range(Span::new(0.001, 0.009)), None);
    }

    #[test]
    fn the_range_of_a_window_is_calibrated() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), -2.0, 1.0);
        assert_eq!(trace.range(Span::new(1.234, 7.89)), Some((-1575.0, -247.0)));
    }

    #[test]
    fn raw_candles_of_a_time_column_hold_the_samples_whose_stamps_fall_in_each_pixel() {
        let tmp = tempfile::tempdir().unwrap();
        let (trace, stamps) = stamped(&tmp.path().join("t.sigrec"), 2000, jittered);
        let (from, to, px) = (3.217, 5.9, 37);
        let mut out = Vec::new();
        trace.candles(Span::new(from, to), px, &mut out);
        assert_eq!(trace.detail((to - from) / px as f64), Detail::Raw);

        let width = (to - from) / px as f64;
        let mut expected = vec![Bin::EMPTY; px];
        for (i, s) in stamps.iter().enumerate() {
            let t = (s - stamps[0]) as f64 * 1e-9;
            let p = ((t - from) / width).floor();
            if p >= 0.0 && p < px as f64 {
                expected[p as usize].include(i as f64);
            }
        }
        assert_eq!(pairs(&out), pairs(&expected));
        assert!(out.iter().all(|b| !b.is_empty()));
    }

    #[test]
    fn the_range_of_a_window_on_a_time_column_is_exact() {
        let tmp = tempfile::tempdir().unwrap();
        let (trace, stamps) = stamped(&tmp.path().join("t.sigrec"), 5000, jittered);
        for (from, to) in [(0.0, 50.0), (1.234, 37.89), (3.0, 3.005), (-1.0, 0.0)] {
            let inside: Vec<f64> = (0..stamps.len())
                .filter(|&i| {
                    let t = (stamps[i] - stamps[0]) as f64 * 1e-9;
                    t >= from && t < to
                })
                .map(|i| i as f64)
                .collect();
            let expected = (!inside.is_empty()).then(|| (inside[0], inside[inside.len() - 1]));
            assert_eq!(trace.range(Span::new(from, to)), expected, "{from}..{to}");
        }
    }

    #[test]
    fn a_hole_longer_than_the_base_bin_is_a_gap_at_every_zoom() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 Hz with a 5 s hole from 20 s to 25 s.
        let (trace, _) = stamped(&tmp.path().join("t.sigrec"), 5000, |i| {
            let i = if i >= 2000 { i + 500 } else { i } as i64;
            i * 10_000_000
        });
        let mut out = Vec::new();
        // From raw candles to the coarsest level: some pixel inside the hole
        // is empty, and the pixels either side of it are not.
        for px in [5000, 1000, 100, 40, 30] {
            trace.candles(Span::new(0.0, 55.0), px, &mut out);
            let pixel = |t: f64| (t / 55.0 * px as f64) as usize;
            assert!(out[pixel(22.5)].is_empty(), "{px} px: no gap");
            assert!(!out[pixel(15.0)].is_empty() && !out[pixel(30.0)].is_empty());
        }
    }

    #[test]
    fn the_line_through_raw_samples_breaks_where_a_sample_is_missing_and_only_there() {
        let tmp = tempfile::tempdir().unwrap();
        // Sample 50 is dropped; the rest jitter by up to ±2 ms.
        let (trace, _) = stamped(&tmp.path().join("t.sigrec"), 100, |i| {
            jittered(if i >= 50 { i + 1 } else { i })
        });
        let mut out = Vec::new();
        trace.samples(Span::new(0.3, 0.7), &mut out);
        let breaks: Vec<f64> = out.iter().filter(|s| s.1.is_nan()).map(|s| s.0).collect();
        assert_eq!(breaks.len(), 1, "{out:?}");
        assert!((breaks[0] - 0.5).abs() < 0.005, "break at {}", breaks[0]);
        assert!(trace.draws_samples(0.019) && !trace.draws_samples(0.02));
    }

    #[test]
    fn k_and_shift_stretch_and_move_the_stream_in_display_time() {
        let tmp = tempfile::tempdir().unwrap();
        let (plain, _) = stamped(&tmp.path().join("a.sigrec"), 3000, jittered);
        let (moved, _) = stamped(&tmp.path().join("b.sigrec"), 3000, jittered);
        let moved = moved.placed(Placement {
            start_s: 0.5,
            k: 2.0,
            shift_s: -3.0,
        });
        // Local t displays at 2 · (0.5 + t) − 3.
        let at = |t: f64| 2.0 * (0.5 + t) - 3.0;
        let span = moved.display_span().unwrap();
        assert_eq!(span.from, -2.0);
        assert!((span.to - at(plain.display_span().unwrap().to)).abs() < 1e-12);

        let (mut a, mut b) = (Vec::new(), Vec::new());
        for (from, to, px) in [(1.0, 9.0, 800), (0.0, 30.0, 30), (2.0, 2.4, 10)] {
            plain.candles(Span::new(from, to), px, &mut a);
            moved.candles(Span::new(at(from), at(to)), px, &mut b);
            assert_eq!(pairs(&a), pairs(&b), "{from}..{to} over {px} px");
            assert_eq!(
                plain.range(Span::new(from, to)),
                moved.range(Span::new(at(from), at(to)))
            );
        }
        let mut sa = Vec::new();
        let mut sb = Vec::new();
        plain.samples(Span::new(2.0, 2.1), &mut sa);
        moved.samples(Span::new(at(2.0), at(2.1)), &mut sb);
        assert_eq!(sa.len(), sb.len());
        for ((ta, va), (tb, vb)) in sa.iter().zip(&sb) {
            assert!((at(*ta) - tb).abs() < 1e-9 && va == vb);
        }
        // Stretched by k, one pixel covers half the samples.
        assert_eq!(moved.samples_per_px(0.02), 1.0);
    }

    #[test]
    fn the_utc_at_a_display_instant_counts_from_the_anchor_on_the_streams_clock() {
        let tmp = tempfile::tempdir().unwrap();
        let (trace, stamps) = stamped(&tmp.path().join("t.sigrec"), 100, |i| i as i64 * 10_000_000);
        let noon = 1_793_275_200_000_000_000;
        // The anchor is 1 s before the first stamp; the stream starts 2 s
        // after the recording's zero and is shifted by 0.25 s.
        let trace = trace
            .placed(Placement {
                start_s: 2.0,
                k: 1.0,
                shift_s: 0.25,
            })
            .anchored(Some(UtcAnchor::new(stamps[0] - 1_000_000_000, noon)));
        assert_eq!(trace.utc_ns_at(2.25), Some(noon + 1_000_000_000));
        assert_eq!(trace.utc_ns_at(2.75), Some(noon + 1_500_000_000));
        let unanchored = trace.anchored(None);
        assert_eq!(unanchored.utc_ns_at(2.25), None);
    }

    #[test]
    fn the_reading_at_an_instant_is_the_nearest_sample_exactly_as_stored_after_calibration() {
        let tmp = tempfile::tempdir().unwrap();
        let trace = ramp(&tmp.path().join("r.sigrec"), -2.0, 1.0);
        let r = trace.nearest(0.123).unwrap();
        assert_eq!((r.index, r.time, r.value, r.stamp), (12, 0.12, -23.0, None));
        assert_eq!(trace.nearest(0.126).unwrap().index, 13);
        assert_eq!(trace.nearest(-0.004).unwrap().index, 0);
        assert_eq!(trace.nearest(10.0).unwrap().index, 999);
        // More than an interval outside the signal: no sample is near.
        assert!(trace.nearest(-0.02).is_none() && trace.nearest(10.02).is_none());
    }

    #[test]
    fn the_reading_on_a_time_column_is_the_sample_with_the_nearest_stamp_and_carries_the_raw_stamp()
    {
        let tmp = tempfile::tempdir().unwrap();
        let (trace, stamps) = stamped(&tmp.path().join("t.sigrec"), 2000, jittered);
        let trace = trace.placed(Placement {
            start_s: 1.0,
            k: 1.0,
            shift_s: 0.5,
        });
        let local = |i: usize| (stamps[i] - stamps[0]) as f64 * 1e-9;
        for t in [1.5, 1.5031, 4.2049, 7.777, 21.48] {
            let closest = (0..stamps.len())
                .min_by(|&a, &b| {
                    let d = |i| (local(i) + 1.5 - t).abs();
                    d(a).total_cmp(&d(b))
                })
                .unwrap();
            let r = trace.nearest(t).unwrap();
            assert_eq!(r.index, closest, "at {t}");
            assert_eq!(r.value, closest as f64);
            assert_eq!(r.stamp, Some(stamps[closest]));
            assert!((r.time - (local(closest) + 1.5)).abs() < 1e-12);
        }
    }

    #[test]
    fn inside_a_hole_the_reading_is_the_nearest_real_sample_either_side() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 Hz with samples 50..=54 dropped: a hole from 0.49 s to 0.55 s.
        let (trace, _) = stamped(&tmp.path().join("t.sigrec"), 100, |i| {
            let i = if i >= 50 { i + 5 } else { i } as i64;
            i * 10_000_000
        });
        assert_eq!(trace.nearest(0.51).unwrap().index, 49);
        assert_eq!(trace.nearest(0.53).unwrap().index, 50);
        assert_eq!(trace.nearest(0.53).unwrap().time, 0.55);
    }

    #[test]
    fn a_reading_prints_as_the_stored_sample_and_nothing_more() {
        let single = |value: f64| Reading {
            index: 0,
            time: 0.0,
            value,
            stamp: None,
            single: true,
        };
        let double = |value: f64| Reading {
            single: false,
            ..single(value)
        };
        // An f32 sample, as stored and after calibration in f64.
        assert_eq!(single(0.1f32 as f64).value_text(), "0.1");
        assert_eq!(single(2.0 * 0.1f32 as f64 + 1.0).value_text(), "1.2");
        assert_eq!(single(9.81f32 as f64).value_text(), "9.81");
        // Wider samples keep 15 significant digits, enough to tell any two
        // apart, and lose the noise of the calibration's arithmetic.
        assert_eq!(double(0.1 + 0.2).value_text(), "0.3");
        assert_eq!(double(0.001 * 1234.0).value_text(), "1.234");
        assert_eq!(double(123456789012.0).value_text(), "123456789012");
        assert_eq!(double(-0.0).value_text(), "0");
        assert_eq!(double(1e-7 / 3.0).value_text(), "3.33333333333333e-8");
    }

    #[test]
    fn a_residual_trace_draws_the_residual_of_its_time_column() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("t.sigrec");
        // Every stamp 1 µs late against 100 Hz, so the residual is 1 µs · i.
        let (_, stamps) = stamped(&dir, 640, |i| i as i64 * 10_001_000);
        let rec = Recording::open(&dir).unwrap();
        let residual = Residual::against_nominal(stamps[0], 1e-9, 100.0);
        let pyramid = rec
            .residual_pyramid("s", &residual, &mut |_, _| {})
            .unwrap();
        let trace = Trace::new(
            Values::Residual(residual),
            Clock::stamped(rec.map_time("s").unwrap().unwrap(), 1e-9, 100.0),
            pyramid,
        )
        .calibrated(1e3, 0.0);
        let (lo, hi) = trace.whole_signal().unwrap();
        assert!(
            lo.abs() < 1e-9 && (hi - 0.639).abs() < 1e-6,
            "{lo}..{hi} ms"
        );
        let mut out = Vec::new();
        trace.samples(Span::new(1.0, 1.0), &mut out);
        assert!((out[1].1 - 0.1).abs() < 1e-9, "{:?}", out[1]);
    }
}
