//! The dynamic string table (`.dynstr`) being edited.

/// `.dynstr` as the original section bytes plus strings appended to it.
///
/// Offsets of existing strings never change, so every reference into the
/// table stays valid while strings are added.
#[derive(Debug, Clone)]
pub(crate) struct DynStr {
    pub section: usize,
    /// File offset of the original section data.
    pub file_offset: u64,
    /// Size of the original section data.
    pub orig_len: usize,
    /// Strings appended after the original data, each NUL-terminated.
    pub extra: Vec<u8>,
}

impl DynStr {
    pub fn len(&self) -> usize {
        self.orig_len + self.extra.len()
    }

    fn orig<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        let start = self.file_offset as usize;
        &data[start..start + self.orig_len]
    }

    /// The NUL-terminated string at `offset`, without its terminator.
    pub fn get<'a>(&'a self, data: &'a [u8], offset: u64) -> Option<&'a [u8]> {
        let offset = usize::try_from(offset).ok()?;
        let rest = if offset < self.orig_len {
            &self.orig(data)[offset..]
        } else {
            self.extra.get(offset - self.orig_len..)?
        };
        let end = rest.iter().position(|&b| b == 0)?;
        Some(&rest[..end])
    }

    /// An offset at which `s` is stored, also as the suffix of a longer string.
    pub fn find(&self, data: &[u8], s: &[u8]) -> Option<u64> {
        // A string never contains NUL, so `s` followed by NUL is found exactly
        // where a string ends with `s`, and the first match is the first
        // such string.
        if s.contains(&0) {
            return None;
        }
        let mut needle = Vec::with_capacity(s.len() + 1);
        needle.extend_from_slice(s);
        needle.push(0);
        let finder = memchr::memmem::Finder::new(&needle);
        if let Some(offset) = finder.find(self.orig(data)) {
            return Some(offset as u64);
        }
        finder
            .find(&self.extra)
            .map(|offset| (self.orig_len + offset) as u64)
    }

    /// The offset of `s`, appending it if it is not stored yet.
    pub fn add(&mut self, data: &[u8], s: &[u8]) -> u64 {
        if let Some(offset) = self.find(data, s) {
            return offset;
        }
        let offset = self.len() as u64;
        self.extra.extend_from_slice(s);
        self.extra.push(0);
        offset
    }

    /// The full table contents: the original bytes followed by appended strings.
    pub fn contents(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(self.orig(data));
        out.extend_from_slice(&self.extra);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(bytes: &[u8]) -> (Vec<u8>, DynStr) {
        let mut data = vec![0xee; 4];
        data.extend_from_slice(bytes);
        let table = DynStr {
            section: 1,
            file_offset: 4,
            orig_len: bytes.len(),
            extra: Vec::new(),
        };
        (data, table)
    }

    #[test]
    fn finds_whole_strings_and_suffixes() {
        let (data, table) = table(b"\0libfoo.so\0bar\0");
        assert_eq!(table.find(&data, b"libfoo.so"), Some(1));
        assert_eq!(table.find(&data, b"foo.so"), Some(4));
        assert_eq!(table.find(&data, b"bar"), Some(11));
        assert_eq!(table.find(&data, b""), Some(0));
        assert_eq!(table.find(&data, b"baz"), None);
        assert_eq!(table.find(&data, b"libfoo"), None);
    }

    #[test]
    fn finds_only_terminated_strings() {
        let (data, table) = table(b"\0foo\0bar");
        assert_eq!(table.find(&data, b"bar"), None);
        assert_eq!(table.find(&data, b"foo\0bar"), None);
        assert_eq!(table.find(&data, b"\0foo"), None);
        assert_eq!(table.find(&data, b"oo"), Some(2));
    }

    #[test]
    fn appends_only_missing_strings() {
        let (data, mut table) = table(b"\0libfoo.so\0");
        assert_eq!(table.add(&data, b"foo.so"), 4);
        assert_eq!(table.add(&data, b"/opt/lib"), 11);
        assert_eq!(table.add(&data, b"lib"), 16);
        assert_eq!(table.len(), 20);
        assert_eq!(table.get(&data, 11), Some(&b"/opt/lib"[..]));
        assert_eq!(table.get(&data, 1), Some(&b"libfoo.so"[..]));
        assert_eq!(table.contents(&data), b"\0libfoo.so\0/opt/lib\0".to_vec());
    }
}
