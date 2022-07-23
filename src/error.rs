use std::{
    io,
    num::NonZeroU32,
};
use crate::{
    dxvk::{HeaderError, EntryError},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("State cache version mismatch: expected v{expected}, found v{found}")]
    VersionMismatch {
        expected: NonZeroU32,
        found: NonZeroU32,
    },
    #[error("No valid state cache entries found")]
    NoEntriesFound,
    #[error("Error reading header: {0}")]
    ReadHeader(#[from] HeaderError),
    #[error("Error reading entry: {0}")]
    ReadEntry(#[from] EntryError),
}

impl Error {
    pub const fn version_mismatch(expected: NonZeroU32, found: NonZeroU32) -> Self {
        Error::VersionMismatch {
            expected: expected,
            found: found,
        }
    }
}
