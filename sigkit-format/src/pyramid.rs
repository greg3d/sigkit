//! The pyramid: min/max decimation levels of one channel, binned by time.
//!
//! The base level holds one bin per [`BASE_BIN_INTERVALS`] nominal intervals;
//! each level above merges two bins of the level below. A bin covering no
//! sample is empty and stays a gap at every level until a sibling fills it.
//! Bins are never averages: a single spike survives to the top level.

use crate::column::Dtype;

/// Nominal intervals covered by one base bin.
pub const BASE_BIN_INTERVALS: u32 = 64;

/// The min and max of the samples a bin covers; `NaN` in both when it covers none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bin {
    pub min: f32,
    pub max: f32,
}

impl Bin {
    /// A bin covering no sample: a gap.
    pub const EMPTY: Bin = Bin {
        min: f32::NAN,
        max: f32::NAN,
    };

    /// True when the bin covers no sample.
    pub fn is_empty(&self) -> bool {
        self.min.is_nan()
    }

    /// Widen to cover `value`. A `NaN` sample is a gap and is not counted.
    pub fn include(&mut self, value: f64) {
        if value.is_nan() {
            return;
        }
        let value = value as f32;
        if self.is_empty() {
            *self = Bin {
                min: value,
                max: value,
            };
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
    }

    /// The bin covering both. An empty side contributes nothing.
    pub fn merge(self, other: Bin) -> Bin {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => Bin {
                min: self.min.min(other.min),
                max: self.max.max(other.max),
            },
        }
    }
}

/// Min/max levels of one channel. Level 0 is the base; the last level is a
/// single bin covering the whole signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Pyramid {
    base_bin_s: f64,
    levels: Vec<Vec<Bin>>,
}

impl Pyramid {
    /// Build from samples already placed in base bins. `bins` is the number
    /// of base bins the signal spans; a sample placed beyond it is dropped,
    /// so one corrupt placement can never grow the pyramid.
    pub fn build(
        base_bin_s: f64,
        bins: usize,
        samples: impl IntoIterator<Item = (usize, f64)>,
    ) -> Pyramid {
        let mut base = vec![Bin::EMPTY; bins];
        for (bin, value) in samples {
            if let Some(slot) = base.get_mut(bin) {
                slot.include(value);
            }
        }
        let mut levels = vec![base];
        while levels.last().is_some_and(|level| level.len() > 1) {
            let below = levels.last().expect("just checked");
            let merged: Vec<Bin> = below
                .chunks(2)
                .map(|pair| pair.iter().copied().fold(Bin::EMPTY, Bin::merge))
                .collect();
            levels.push(merged);
        }
        Pyramid { base_bin_s, levels }
    }

    /// Build for a fixed-rate stream of `n` samples: sample `i` sits at
    /// `i / rate_hz`, so base bin `k` covers samples `64k..64k+64`.
    pub fn build_fixed_rate(
        rate_hz: f64,
        n: usize,
        mut value: impl FnMut(usize) -> f64,
    ) -> Pyramid {
        let base = BASE_BIN_INTERVALS as usize;
        Pyramid::build(
            base as f64 / rate_hz,
            n.div_ceil(base),
            (0..n).map(|i| (i / base, value(i))),
        )
    }

    /// Seconds covered by one base bin: 64 nominal intervals.
    pub fn base_bin_s(&self) -> f64 {
        self.base_bin_s
    }

    /// Seconds covered by one bin of `level`.
    pub fn bin_s(&self, level: usize) -> f64 {
        self.base_bin_s * (1u64 << level) as f64
    }

    /// Number of levels, the base included.
    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    /// The bins of `level`; level 0 is the base.
    pub fn level(&self, level: usize) -> &[Bin] {
        &self.levels[level]
    }

    /// The min and max over the whole signal; `None` when it has no sample.
    pub fn whole_signal(&self) -> Option<(f32, f32)> {
        let top = self.levels.last()?.first()?;
        (!top.is_empty()).then_some((top.min, top.max))
    }
}

// ---- Cache file: `.cache/<stream>.<channel>.pyr` inside the recording ----

/// What a cached pyramid was built from. A cache whose fingerprint differs
/// from the columns on disk is stale and is rebuilt.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    /// `(length in bytes, modification time in ns since the Unix epoch)` of
    /// every column the pyramid depends on: the value column, then the time
    /// column of a timestamped stream.
    pub files: Vec<(u64, u128)>,
    /// Dtype of the value column.
    pub dtype: Dtype,
    /// Seconds per base bin: 64 nominal intervals.
    pub base_bin_s: f64,
}

const CACHE_MAGIC: &[u8; 8] = b"SKPYR\x01\0\0";

impl Pyramid {
    /// The cache file bytes: magic and version, fingerprint, then every level.
    pub fn to_cache_bytes(&self, fingerprint: &Fingerprint) -> Vec<u8> {
        let bins: usize = self.levels.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity(64 + 24 * fingerprint.files.len() + 8 * bins);
        out.extend_from_slice(CACHE_MAGIC);
        out.extend_from_slice(&(fingerprint.files.len() as u32).to_le_bytes());
        for (len, mtime) in &fingerprint.files {
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&mtime.to_le_bytes());
        }
        out.push(dtype_tag(fingerprint.dtype));
        out.extend_from_slice(&fingerprint.base_bin_s.to_le_bytes());
        out.extend_from_slice(&(self.levels.len() as u32).to_le_bytes());
        for level in &self.levels {
            out.extend_from_slice(&(level.len() as u64).to_le_bytes());
            for bin in level {
                out.extend_from_slice(&bin.min.to_le_bytes());
                out.extend_from_slice(&bin.max.to_le_bytes());
            }
        }
        out
    }

    /// Decode a cache file. `None` when it is not a pyramid cache, was written
    /// for other columns than `fingerprint` describes, or is truncated.
    pub fn from_cache_bytes(bytes: &[u8], fingerprint: &Fingerprint) -> Option<Pyramid> {
        let mut r = Reader(bytes);
        if r.take(8)? != CACHE_MAGIC {
            return None;
        }
        let files = r.u32()? as usize;
        if files != fingerprint.files.len() {
            return None;
        }
        for &(len, mtime) in &fingerprint.files {
            if (r.u64()?, r.u128()?) != (len, mtime) {
                return None;
            }
        }
        if r.u8()? != dtype_tag(fingerprint.dtype) || r.f64()? != fingerprint.base_bin_s {
            return None;
        }
        // Untrusted counts: allocate only what the bytes can actually hold.
        // Each level halves the one below, so 64 levels cover any column.
        let levels = r.u32()? as usize;
        if levels == 0 || levels > 64 {
            return None;
        }
        let mut pyramid = Pyramid {
            base_bin_s: fingerprint.base_bin_s,
            levels: Vec::with_capacity(levels),
        };
        for _ in 0..levels {
            let n = r.u64()? as usize;
            let raw = r.take(n.checked_mul(8)?)?;
            let level: Vec<Bin> = raw
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| Bin {
                    min: f32::from_le_bytes(b[..4].try_into().unwrap()),
                    max: f32::from_le_bytes(b[4..].try_into().unwrap()),
                })
                .collect();
            // Each level merges pairs of the one below; anything else is corrupt.
            if let Some(below) = pyramid.levels.last()
                && level.len() != below.len().div_ceil(2)
            {
                return None;
            }
            pyramid.levels.push(level);
        }
        if r.0.is_empty() && pyramid.levels.last().is_some_and(|top| top.len() <= 1) {
            Some(pyramid)
        } else {
            None
        }
    }
}

fn dtype_tag(dtype: Dtype) -> u8 {
    match dtype {
        Dtype::F32 => 0,
        Dtype::F64 => 1,
        Dtype::I16 => 2,
        Dtype::I32 => 3,
        Dtype::I64 => 4,
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn u128(&mut self) -> Option<u128> {
        self.take(16)
            .map(|b| u128::from_le_bytes(b.try_into().unwrap()))
    }
    fn f64(&mut self) -> Option<f64> {
        self.take(8)
            .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
    }
}
