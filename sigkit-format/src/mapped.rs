//! A column read through a memory map. The OS, not the app, decides which
//! pages stay resident, so a recording larger than RAM pages in as the
//! viewport touches it.

use crate::column::Dtype;
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
        self.element(i).map(|bytes| self.dtype.decode_le(bytes))
    }

    /// The raw sample at `i` as an integer: exact for integer columns (time
    /// columns are `i64`), truncated for float ones.
    pub fn get_i64(&self, i: usize) -> Option<i64> {
        self.element(i).map(|bytes| self.dtype.decode_le_i64(bytes))
    }

    /// Append the samples in `range` (clamped to the column) to `out`, widened
    /// to `f64`. `out` is cleared first and only grows, so a reused buffer does
    /// not allocate once warm.
    pub fn read_into(&self, range: Range<usize>, out: &mut Vec<f64>) {
        out.clear();
        let range = range.start.min(self.len)..range.end.min(self.len);
        let size = self.dtype.size();
        let bytes = &self.bytes()[range.start * size..range.end * size];
        out.extend(bytes.chunks_exact(size).map(|b| self.dtype.decode_le(b)));
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
        Some(&self.bytes()[i * size..(i + 1) * size])
    }
}
