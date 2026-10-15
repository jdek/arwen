//! Rewriting ELF executables and shared objects in place.
//!
//! [`Writer`] reads a file, records modifications, and writes the result.
//! Sections that still fit are rewritten where they are; the rest move into a
//! segment appended to the file (see `layout.rs`).
//!
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod codec;
pub mod consts;
mod error;
mod file;
mod hash;
mod layout;
mod strtab;
mod writer;

pub use error::{ElfError, Result};
pub use writer::Writer;
