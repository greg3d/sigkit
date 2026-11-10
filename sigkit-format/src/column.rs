//! A column is one planar little-endian array on disk: one file, one dtype.

use serde::{Deserialize, Serialize};

/// The element type of a column. Time columns are always `i64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dtype {
    F32,
    F64,
    I16,
    I32,
    I64,
}

impl Dtype {
    /// Bytes per element on disk.
    pub fn size(self) -> usize {
        match self {
            Dtype::F32 | Dtype::I32 => 4,
            Dtype::F64 | Dtype::I64 => 8,
            Dtype::I16 => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Dtype::F32 => "f32",
            Dtype::F64 => "f64",
            Dtype::I16 => "i16",
            Dtype::I32 => "i32",
            Dtype::I64 => "i64",
        }
    }
}

impl Dtype {
    /// Decode one little-endian element, widened to `f64`. `bytes` is exactly
    /// [`Dtype::size`] long.
    pub fn decode_le(self, bytes: &[u8]) -> f64 {
        match self {
            Dtype::F32 => f32::from_le_bytes(bytes.try_into().unwrap()) as f64,
            Dtype::F64 => f64::from_le_bytes(bytes.try_into().unwrap()),
            Dtype::I16 => i16::from_le_bytes(bytes.try_into().unwrap()) as f64,
            Dtype::I32 => i32::from_le_bytes(bytes.try_into().unwrap()) as f64,
            Dtype::I64 => i64::from_le_bytes(bytes.try_into().unwrap()) as f64,
        }
    }

    /// Decode one little-endian element as an integer: exact for integer
    /// dtypes, truncated for floats. `bytes` is exactly [`Dtype::size`] long.
    pub fn decode_le_i64(self, bytes: &[u8]) -> i64 {
        match self {
            Dtype::F32 => f32::from_le_bytes(bytes.try_into().unwrap()) as i64,
            Dtype::F64 => f64::from_le_bytes(bytes.try_into().unwrap()) as i64,
            Dtype::I16 => i16::from_le_bytes(bytes.try_into().unwrap()) as i64,
            Dtype::I32 => i32::from_le_bytes(bytes.try_into().unwrap()) as i64,
            Dtype::I64 => i64::from_le_bytes(bytes.try_into().unwrap()),
        }
    }
}

/// Byte order of a raw file's elements. Recording columns are always little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ByteOrder {
    #[default]
    Little,
    Big,
}

impl ByteOrder {
    pub const ALL: [ByteOrder; 2] = [ByteOrder::Little, ByteOrder::Big];

    pub fn name(self) -> &'static str {
        match self {
            ByteOrder::Little => "little",
            ByteOrder::Big => "big",
        }
    }
}

impl Dtype {
    /// Every dtype, integers first, narrowest first.
    pub const ALL: [Dtype; 5] = [Dtype::I16, Dtype::I32, Dtype::I64, Dtype::F32, Dtype::F64];

    /// Decode one element in `order`, widened to `f64`. `bytes` is exactly
    /// [`Dtype::size`] long.
    pub fn decode(self, bytes: &[u8], order: ByteOrder) -> f64 {
        match order {
            ByteOrder::Little => self.decode_le(bytes),
            ByteOrder::Big => {
                let mut le = [0u8; 8];
                let le = &mut le[..bytes.len()];
                le.copy_from_slice(bytes);
                le.reverse();
                self.decode_le(le)
            }
        }
    }

    /// Encode `value` as one element in `order`: rounded and saturated for
    /// integer dtypes.
    pub fn encode(self, value: f64, order: ByteOrder) -> Vec<u8> {
        let mut le = match self {
            Dtype::F32 => (value as f32).to_le_bytes().to_vec(),
            Dtype::F64 => value.to_le_bytes().to_vec(),
            Dtype::I16 => (value.round() as i16).to_le_bytes().to_vec(),
            Dtype::I32 => (value.round() as i32).to_le_bytes().to_vec(),
            Dtype::I64 => (value.round() as i64).to_le_bytes().to_vec(),
        };
        if order == ByteOrder::Big {
            le.reverse();
        }
        le
    }
}

/// Where one channel's elements sit in its file: the first at
/// `offset_bytes + lane_bytes`, each next one `frame_bytes` further on. A
/// recording's own column is planar: no offset, one element per frame,
/// little-endian. An imported raw file interleaves its channels after a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interleave {
    pub offset_bytes: u64,
    pub frame_bytes: usize,
    pub lane_bytes: usize,
    pub byte_order: ByteOrder,
}

impl Interleave {
    /// A planar little-endian column of `dtype`.
    pub fn planar(dtype: Dtype) -> Interleave {
        Interleave {
            offset_bytes: 0,
            frame_bytes: dtype.size(),
            lane_bytes: 0,
            byte_order: ByteOrder::Little,
        }
    }

    /// Whole frames in a file of `file_bytes`, and the bytes after the last
    /// one. A file shorter than the offset holds no frame.
    pub fn frames(&self, file_bytes: u64) -> (usize, u64) {
        let body = file_bytes.saturating_sub(self.offset_bytes);
        let frame = self.frame_bytes.max(1) as u64;
        ((body / frame) as usize, body % frame)
    }
}

impl std::fmt::Display for Dtype {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One channel's samples in memory, typed as on disk.
#[derive(Debug, Clone, PartialEq)]
pub enum Column {
    F32(Vec<f32>),
    F64(Vec<f64>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
}

impl Column {
    pub fn dtype(&self) -> Dtype {
        match self {
            Column::F32(_) => Dtype::F32,
            Column::F64(_) => Dtype::F64,
            Column::I16(_) => Dtype::I16,
            Column::I32(_) => Dtype::I32,
            Column::I64(_) => Dtype::I64,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Column::F32(v) => v.len(),
            Column::F64(v) => v.len(),
            Column::I16(v) => v.len(),
            Column::I32(v) => v.len(),
            Column::I64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The raw sample at `i`, widened to `f64`. No gain or offset applied.
    pub fn get(&self, i: usize) -> Option<f64> {
        match self {
            Column::F32(v) => v.get(i).map(|x| *x as f64),
            Column::F64(v) => v.get(i).copied(),
            Column::I16(v) => v.get(i).map(|x| *x as f64),
            Column::I32(v) => v.get(i).map(|x| *x as f64),
            Column::I64(v) => v.get(i).map(|x| *x as f64),
        }
    }

    /// The on-disk bytes: planar, little-endian.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        fn pack<T: Copy, const N: usize>(v: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
            v.iter().flat_map(|x| f(*x)).collect()
        }
        match self {
            Column::F32(v) => pack(v, f32::to_le_bytes),
            Column::F64(v) => pack(v, f64::to_le_bytes),
            Column::I16(v) => pack(v, i16::to_le_bytes),
            Column::I32(v) => pack(v, i32::to_le_bytes),
            Column::I64(v) => pack(v, i64::to_le_bytes),
        }
    }

    /// Decode on-disk bytes. `None` when the length is not a multiple of the dtype size.
    pub fn from_le_bytes(dtype: Dtype, bytes: &[u8]) -> Option<Column> {
        if !bytes.len().is_multiple_of(dtype.size()) {
            return None;
        }
        fn unpack<T, const N: usize>(bytes: &[u8], f: impl Fn([u8; N]) -> T) -> Vec<T> {
            bytes.as_chunks::<N>().0.iter().map(|c| f(*c)).collect()
        }
        Some(match dtype {
            Dtype::F32 => Column::F32(unpack(bytes, f32::from_le_bytes)),
            Dtype::F64 => Column::F64(unpack(bytes, f64::from_le_bytes)),
            Dtype::I16 => Column::I16(unpack(bytes, i16::from_le_bytes)),
            Dtype::I32 => Column::I32(unpack(bytes, i32::from_le_bytes)),
            Dtype::I64 => Column::I64(unpack(bytes, i64::from_le_bytes)),
        })
    }
}

/// Decode a time column (`i64`, little-endian). `None` when the length is not a multiple of 8.
pub fn time_from_le_bytes(bytes: &[u8]) -> Option<Vec<i64>> {
    match Column::from_le_bytes(Dtype::I64, bytes)? {
        Column::I64(v) => Some(v),
        _ => unreachable!(),
    }
}

/// Encode a time column (`i64`, little-endian).
pub fn time_to_le_bytes(stamps: &[i64]) -> Vec<u8> {
    stamps.iter().flat_map(|s| s.to_le_bytes()).collect()
}
