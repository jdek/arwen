//! Reading only the parts of a file that an edit needs.
//!
//! A writer opened from a file starts out with a zeroed buffer as long as the
//! file and reads ranges into it as they are needed. Pages of the buffer that
//! are never read or written are never touched, so for a large file with a
//! small change only a few pages are read, instead of the whole file.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

use super::{ElfError, Result};

/// The file a writer reads from, and which of its bytes it has read.
#[derive(Debug)]
pub(super) struct Source {
    file: File,
    pub len: u64,
    /// The ranges read so far, sorted and disjoint.
    loaded: Vec<Range<u64>>,
}

impl Source {
    pub fn new(file: File, len: u64) -> Self {
        Self {
            file,
            len,
            loaded: Vec::new(),
        }
    }

    /// The parts of `range` not read yet.
    pub fn missing(&self, range: Range<u64>) -> Vec<Range<u64>> {
        let mut missing = Vec::new();
        let mut at = range.start;
        // The first loaded range that ends after `at`.
        let first = self.loaded.partition_point(|r| r.end <= at);
        for r in &self.loaded[first..] {
            if r.start >= range.end {
                break;
            }
            if r.start > at {
                missing.push(at..r.start);
            }
            at = at.max(r.end);
        }
        if at < range.end {
            missing.push(at..range.end);
        }
        missing
    }

    /// Record `range` as read.
    pub fn mark(&mut self, range: Range<u64>) {
        if range.is_empty() {
            return;
        }
        let first = self.loaded.partition_point(|r| r.end < range.start);
        let last = self.loaded.partition_point(|r| r.start <= range.end);
        let mut merged = range;
        for r in &self.loaded[first..last] {
            merged.start = merged.start.min(r.start);
            merged.end = merged.end.max(r.end);
        }
        self.loaded.splice(first..last, [merged]);
    }

    pub fn read(&mut self, range: Range<u64>, buf: &mut [u8]) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(range.start))
            .map_err(ElfError::Io)?;
        self.file.read_exact(buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => {
                ElfError::Modify("the file got shorter while it was being read".into())
            }
            _ => ElfError::Io(e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(loaded: &[Range<u64>]) -> Source {
        let mut s = Source::new(any_file(), 100);
        for r in loaded {
            s.mark(r.clone());
        }
        s
    }

    fn any_file() -> File {
        File::open(std::env::current_exe().unwrap()).unwrap()
    }

    #[test]
    fn missing_ranges() {
        let s = source(&[10..20, 30..40]);
        assert_eq!(s.missing(0..50), vec![0..10, 20..30, 40..50]);
        assert_eq!(s.missing(12..18), vec![]);
        assert_eq!(s.missing(15..35), vec![20..30]);
        assert_eq!(s.missing(40..45), vec![40..45]);
        assert_eq!(s.missing(5..10), vec![5..10]);
    }

    #[test]
    fn marking_merges() {
        let mut s = source(&[10..20, 30..40, 50..60]);
        s.mark(15..32);
        assert_eq!(s.loaded, vec![10..40, 50..60]);
        s.mark(40..50);
        assert_eq!(s.loaded, vec![10..60]);
        s.mark(0..5);
        assert_eq!(s.loaded, vec![0..5, 10..60]);
        s.mark(5..10);
        assert_eq!(s.loaded, vec![0..60]);
    }
}
