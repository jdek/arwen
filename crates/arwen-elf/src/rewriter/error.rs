use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
/// An error that occurred while reading, modifying, or writing an ELF file.
pub enum ElfError {
    #[error("I/O error")]
    /// An I/O error occurred while writing the file.
    Io(#[from] io::Error),
    #[error("malformed ELF file: {0}")]
    /// The file is not a valid ELF file.
    Malformed(String),
    #[error("unsupported ELF file: {0}")]
    /// The file is valid, but this kind of file or operation is not supported.
    Unsupported(String),
    #[error("cannot modify the file: {0}")]
    /// The modification cannot be applied to this file.
    Modify(String),
    #[error("cannot find section '{0}'")]
    /// The file has no section that the operation needs.
    MissingSection(&'static str),
}

/// The  `Result` type for this library.
pub type Result<T> = std::result::Result<T, ElfError>;
