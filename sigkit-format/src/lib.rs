//! The `.sigrec` recording format.
//!
//! A recording is a directory holding `header.json` and one planar
//! little-endian column file per channel (plus one time column per
//! timestamped stream). Raw column bytes are never rewritten; only the
//! header is, atomically. See `GLOSSARY.md` at the repository root.

mod atomic;
pub mod column;
pub mod error;
pub mod header;
pub mod ratefit;
pub mod recording;
pub mod synth;
pub mod wallclock;

pub use column::{Column, Dtype};
pub use error::{Error, Result};
pub use header::{
    Channel, ClockDomain, Header, Stream, StreamKind, TIME_COLUMN, TimeCorrection, UtcAnchor,
};
pub use ratefit::{RateFit, fit_rate, ppm_from_nominal};
pub use recording::{Recording, StreamData};
