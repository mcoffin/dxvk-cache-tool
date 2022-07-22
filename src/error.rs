use std::{
    io,
    num::NonZeroU32,
};

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Magic string mismatch")]
    MagicStringMismatch,
    #[error("Header contained invalid zero version")]
    InvalidVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("File extension mismatch: found: {0:?}, expected .dxvk-cache")]
    InvalidInputExtension(Option<String>),
    #[error("State cache version mismatch: expected v{expected}, found v{found}")]
    VersionMismatch {
        expected: NonZeroU32,
        found: NonZeroU32,
    },
    #[error("No valid state cache entries found")]
    NoEntriesFound,
    #[error("Error reading header: {0}")]
    ReadHeader(#[from] HeaderError),
}

impl Error {
    pub const fn version_mismatch(expected: NonZeroU32, found: NonZeroU32) -> Self {
        Error::VersionMismatch {
            expected: expected,
            found: found,
        }
    }

    #[inline(always)]
    pub fn invalid_input_extension<S>(found: S) -> Self where
        S: Into<Option<String>>,
    {
        Error::InvalidInputExtension(found.into())
    }
}
