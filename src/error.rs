//! Error types shared across the crate.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure.
    Io(io::Error),
    /// Input does not conform to the LZF1 format (bad magic, bad tables,
    /// out-of-range distances, CRC mismatch, truncation, ...).
    Corrupt(&'static str),
    /// The input is a valid stream but its trailing checksum does not match.
    ChecksumMismatch { expected: u32, actual: u32 },
    /// Output file already exists (use `--force`).
    Exists(String),
    /// Command line misuse.
    Usage(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{}", e),
            Error::Corrupt(what) => write!(f, "corrupt input: {}", what),
            Error::ChecksumMismatch { expected, actual } => write!(
                f,
                "checksum mismatch (stored {:08x}, computed {:08x})",
                expected, actual
            ),
            Error::Exists(path) => write!(f, "{} already exists (use -f to overwrite)", path),
            Error::Usage(msg) => write!(f, "{}", msg),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
