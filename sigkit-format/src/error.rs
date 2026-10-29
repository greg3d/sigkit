use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: a recording already exists here")]
    AlreadyExists { path: PathBuf },
    #[error("unsupported sigrec version {0} (this crate reads version 1)")]
    UnsupportedVersion(u32),
    #[error("stream `{stream}`: {reason}")]
    InvalidStream { stream: String, reason: String },
    #[error("header lists {header} streams but {given} were given")]
    StreamCountMismatch { header: usize, given: usize },
    #[error("stream `{stream}`: column `{column}` is missing ({path})")]
    MissingColumn {
        stream: String,
        column: String,
        path: PathBuf,
    },
    #[error(
        "stream `{stream}`: column `{column}` holds {bytes} bytes, not a multiple of {dtype} ({path})"
    )]
    ColumnSize {
        stream: String,
        column: String,
        bytes: u64,
        dtype: &'static str,
        path: PathBuf,
    },
    #[error("stream `{stream}`: columns disagree in length ({detail})")]
    ColumnLengthMismatch { stream: String, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;
