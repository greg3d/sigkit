//! A trace is one channel ready to draw: its memory-mapped column, its
//! pyramid, its rate and calibration. It answers one question per frame:
//! for these pixels along time, what is the min and max in each?

use crate::view::Span;
use sigkit_format::{BASE_BIN_INTERVALS, Bin, MappedColumn, Pyramid};

/// Tolerance, in pixels (or samples), when deciding which pixel a sample or
/// bin falls in: a value on a boundary belongs to the pixel that starts there
/// even when floating point puts it a hair below.
const EPS: f64 = 1e-6;

/// How a viewport is drawn at a given time-per-pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// From this pyramid level: the coarsest whose bin fits in one pixel.
    Level(usize),
    /// Below the base bin: straight from the memory-mapped column.
    Raw,
}

/// One fixed-rate channel ready to draw.
pub struct Trace {
    column: MappedColumn,
    pyramid: Pyramid,
    rate_hz: f64,
    gain: f64,
    offset: f64,
}

impl Trace {
    pub fn new(
        column: MappedColumn,
        pyramid: Pyramid,
        rate_hz: f64,
        gain: f64,
        offset: f64,
    ) -> Trace {
        Trace {
            column,
            pyramid,
            rate_hz,
            gain,
            offset,
        }
    }

    #[cfg(test)]
    pub fn pyramid(&self) -> &Pyramid {
        &self.pyramid
    }

    pub fn len(&self) -> usize {
        self.column.len()
    }

    pub fn rate_hz(&self) -> f64 {
        self.rate_hz
    }

    /// Seconds from the first sample to one interval past the last.
    pub fn duration_s(&self) -> f64 {
        self.len() as f64 / self.rate_hz
    }

    /// Min and max of the whole signal in display units; `None` when empty.
    pub fn whole_signal(&self) -> Option<(f64, f64)> {
        self.pyramid
            .whole_signal()
            .map(|(lo, hi)| self.calibrated_pair(lo as f64, hi as f64))
    }

    /// Raw samples one pixel covers at this time-per-pixel.
    pub fn samples_per_px(&self, time_per_px: f64) -> f64 {
        time_per_px * self.rate_hz
    }

    pub fn detail(&self, time_per_px: f64) -> Detail {
        if time_per_px < self.pyramid.base_bin_s() {
            return Detail::Raw;
        }
        let mut level = 0;
        while level + 1 < self.pyramid.levels() && self.pyramid.bin_s(level + 1) <= time_per_px {
            level += 1;
        }
        Detail::Level(level)
    }

    /// One candle per pixel for `px` pixels covering `time`, in display
    /// units. A pixel covering no sample is an empty bin: a gap. Above the
    /// base every bin goes to the pixel holding its centre, so every sample
    /// is drawn exactly once; below it, raw samples go to the pixel holding
    /// their instant.
    pub fn candles(&self, time: Span, px: usize, out: &mut Vec<Bin>) {
        out.clear();
        out.resize(px, Bin::EMPTY);
        if px == 0 || !time.len().is_finite() || time.len() <= 0.0 {
            return;
        }
        let time_per_px = time.len() / px as f64;
        let pixel_of = |t: f64| -> Option<usize> {
            let p = ((t - time.from) / time_per_px + EPS).floor();
            (p >= 0.0 && p < px as f64).then_some(p as usize)
        };
        match self.detail(time_per_px) {
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
                let first = (time.from * self.rate_hz - EPS).ceil().max(0.0) as usize;
                let end = ((time.to * self.rate_hz - EPS).ceil().max(0.0) as usize).min(self.len());
                for i in first..end {
                    if let Some(p) = pixel_of(i as f64 / self.rate_hz) {
                        out[p].include(self.column.get(i).expect("i < len"));
                    }
                }
            }
        }
        for bin in out.iter_mut() {
            *bin = self.calibrated(*bin);
        }
    }

    /// `(instant, display value)` of every sample in `time`, plus one
    /// neighbour on each side so a line through them enters and leaves the
    /// window. For when a pixel covers less than one sample.
    pub fn samples(&self, time: Span, out: &mut Vec<(f64, f64)>) {
        out.clear();
        if self.len() == 0 {
            return;
        }
        let first = ((time.from * self.rate_hz - EPS).ceil() as i64 - 1).max(0) as usize;
        let last = ((time.to * self.rate_hz + EPS).floor() as i64 + 1).max(0) as usize;
        let last = last.min(self.len() - 1);
        for i in first..=last {
            let raw = self.column.get(i).expect("i < len");
            out.push((i as f64 / self.rate_hz, self.gain * raw + self.offset));
        }
    }

    /// The exact min and max, in display units, of the samples whose instant
    /// lies in `[time.from, time.to)`; `None` when there is none. Partial
    /// base bins at the edges are read raw (at most 126 samples); whole bins
    /// between them come from the pyramid, coarsest first, so the cost is
    /// logarithmic in the window.
    pub fn range(&self, time: Span) -> Option<(f64, f64)> {
        let at = |t: f64| ((t * self.rate_hz - EPS).ceil().max(0.0) as usize).min(self.len());
        let (first, end) = (at(time.from), at(time.to));
        if first >= end {
            return None;
        }
        let base = BASE_BIN_INTERVALS as usize;
        let mut all = Bin::EMPTY;
        let mut raw = |range: std::ops::Range<usize>| {
            for i in range {
                all.include(self.column.get(i).expect("i < len"));
            }
        };
        let (mut a, mut b) = (first.div_ceil(base), end / base);
        if a >= b {
            raw(first..end);
        } else {
            raw(first..a * base);
            raw(b * base..end);
            let mut level = 0;
            while a < b {
                let bins = self.pyramid.level(level);
                if a % 2 == 1 {
                    all = all.merge(bins[a]);
                    a += 1;
                }
                if b % 2 == 1 {
                    b -= 1;
                    all = all.merge(bins[b]);
                }
                (a, b, level) = (a / 2, b / 2, level + 1);
            }
        }
        (!all.is_empty()).then(|| self.calibrated_pair(all.min as f64, all.max as f64))
    }

    fn calibrated(&self, bin: Bin) -> Bin {
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
    use sigkit_format::{Channel, Column, Dtype, Header, Recording, Stream, StreamData};

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
        Trace::new(column, pyramid, 100.0, gain, offset)
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
        assert_eq!(trace.duration_s(), 10.0);
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
}
