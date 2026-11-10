//! A column read through a memory map. The OS, not the app, decides which
//! pages stay resident, so a recording larger than RAM pages in as the
//! viewport touches it.

use crate::column::{ByteOrder, Column, Dtype, Interleave};
use memmap2::{Advice, Mmap};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;

/// One channel's column on disk, mapped read-only and decoded on access.
#[derive(Debug)]
pub struct MappedColumn {
    /// `None` for an empty column: a zero-length file cannot be mapped.
    map: Option<Mmap>,
    dtype: Dtype,
    len: usize,
    interleave: Interleave,
}

impl MappedColumn {
    /// Map `path` as a column of `dtype`. Fails when the file size is not a
    /// whole number of elements.
    pub fn open(path: &Path, dtype: Dtype) -> io::Result<MappedColumn> {
        let file = File::open(path)?;
        let bytes = file.metadata()?.len();
        let size = dtype.size() as u64;
        if bytes % size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{bytes} bytes is not a whole number of {dtype} elements"),
            ));
        }
        let map = if bytes == 0 {
            None
        } else {
            // SAFETY: the map is read-only and this crate never writes column
            // bytes. The format promises raw columns are never rewritten while a
            // recording is open; that promise is what this relies on. If another
            // process truncates the file while it is mapped, touching the lost
            // pages raises SIGBUS and the process dies, and bytes changed in
            // place are seen as they change. Appending is safe: the map keeps
            // the length it was opened with.
            Some(unsafe { Mmap::map(&file)? })
        };
        Ok(MappedColumn {
            map,
            dtype,
            len: (bytes / size) as usize,
            interleave: Interleave::planar(dtype),
        })
    }

    /// Map one channel of `dtype` read in place from a file laid out as
    /// `interleave` says: as many samples as the file holds whole frames,
    /// bytes after the last one left out (unlike [`MappedColumn::open`], which
    /// refuses them).
    pub fn open_interleaved(
        path: &Path,
        dtype: Dtype,
        interleave: Interleave,
    ) -> io::Result<MappedColumn> {
        if interleave.lane_bytes + dtype.size() > interleave.frame_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the channel does not fit in its frame",
            ));
        }
        let file = File::open(path)?;
        let (len, _) = interleave.frames(file.metadata()?.len());
        // SAFETY: as in `open`; an imported raw file is never written either.
        let map = if len == 0 {
            None
        } else {
            Some(unsafe { Mmap::map(&file)? })
        };
        Ok(MappedColumn {
            map,
            dtype,
            len,
            interleave,
        })
    }

    /// The element type on disk.
    pub fn dtype(&self) -> Dtype {
        self.dtype
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the column holds no sample.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The raw sample at `i`, widened to `f64`. No gain or offset applied.
    pub fn get(&self, i: usize) -> Option<f64> {
        self.element(i)
            .map(|bytes| self.dtype.decode(bytes, self.interleave.byte_order))
    }

    /// The raw sample at `i` as an integer: exact for integer columns (time
    /// columns are `i64`), truncated for float ones.
    pub fn get_i64(&self, i: usize) -> Option<i64> {
        debug_assert_eq!(self.interleave, Interleave::planar(self.dtype));
        self.element(i).map(|bytes| self.dtype.decode_le_i64(bytes))
    }

    /// Append the samples in `range` (clamped to the column) to `out`, widened
    /// to `f64`. `out` is cleared first and only grows, so a reused buffer does
    /// not allocate once warm.
    pub fn read_into(&self, range: Range<usize>, out: &mut Vec<f64>) {
        out.clear();
        let range = range.start.min(self.len)..range.end.min(self.len);
        let size = self.dtype.size();
        if self.interleave == Interleave::planar(self.dtype) {
            let bytes = &self.bytes()[range.start * size..range.end * size];
            out.extend(bytes.chunks_exact(size).map(|b| self.dtype.decode_le(b)));
        } else {
            out.extend(range.map(|i| self.get(i).expect("in range")));
        }
    }

    /// The whole column in memory, typed as on disk.
    pub fn to_column(&self) -> Column {
        let order = self.interleave.byte_order;
        let le = |i: usize| -> Vec<u8> {
            let mut bytes = self.element(i).expect("i < len").to_vec();
            if order == ByteOrder::Big {
                bytes.reverse();
            }
            bytes
        };
        let mut bytes = Vec::with_capacity(self.len * self.dtype.size());
        for i in 0..self.len {
            bytes.extend_from_slice(&le(i));
        }
        Column::from_le_bytes(self.dtype, &bytes).expect("whole elements")
    }

    /// Tell the OS the column is about to be read front to back.
    pub fn advise_sequential(&self) {
        if let Some(map) = &self.map {
            let _ = map.advise(Advice::Sequential);
        }
    }

    /// Tell the OS accesses are scattered again (a viewport paging around).
    pub fn advise_random(&self) {
        if let Some(map) = &self.map {
            let _ = map.advise(Advice::Random);
        }
    }

    fn bytes(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }

    fn element(&self, i: usize) -> Option<&[u8]> {
        if i >= self.len {
            return None;
        }
        let size = self.dtype.size();
        let Interleave {
            offset_bytes,
            frame_bytes,
            lane_bytes,
            ..
        } = self.interleave;
        let start = offset_bytes as usize + i * frame_bytes + lane_bytes;
        Some(&self.bytes()[start..start + size])
    }
}
