//! Editing an ELF file in place.
//!
//! The file is kept as its original bytes. Edits are recorded against decoded
//! copies of the few structures whose size can change (the dynamic table, the
//! dynamic string table, the interpreter, the program headers), or written
//! straight into the bytes when their size cannot change (symbol names,
//! versions). `commit` (see `layout.rs`) then writes everything back, keeping
//! every section that still fits where it is.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{self, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
};

use super::codec::{slice, Codec, Dyn, ProgramHeader, Sym};
use super::consts::*;
use super::file::ElfFile;
use super::strtab::DynStr;
use super::{ElfError, Result};

/// Dynamic tags whose value is an offset into `.dynstr`.
pub(super) const STRING_TAGS: [i64; 9] = [
    DT_NEEDED,
    DT_SONAME,
    DT_RPATH,
    DT_RUNPATH,
    DT_AUXILIARY,
    DT_FILTER,
    DT_AUDIT,
    DT_DEPAUDIT,
    DT_CONFIG,
];

/// The dynamic table being edited.
#[derive(Debug, Clone)]
pub(super) struct DynamicTable {
    pub section: usize,
    /// The entries before the terminating `DT_NULL`.
    pub entries: Vec<Dyn>,
    /// For `DT_MIPS_RLD_MAP_REL`, whose value is relative to the entry's own
    /// address: the absolute address it refers to.
    pub rld_map_rel_target: Option<u64>,
}

/// A rewriter for ELF executables and shared objects.
///
/// It reads a file, applies modifications, and writes the result back.
pub struct Writer<'data> {
    pub(super) data: Cow<'data, [u8]>,
    pub(super) file: ElfFile,
    /// The program headers to write. May hold more entries than the file has.
    pub(super) phdrs: Vec<ProgramHeader>,
    pub(super) os_abi: u8,
    pub(super) dynamic: Option<DynamicTable>,
    pub(super) dynstr: Option<DynStr>,
    /// New contents of `.interp`, including the NUL terminator.
    pub(super) interp: Option<Vec<u8>>,
    /// Whether dynamic symbols were renamed, so the hash tables must be rebuilt.
    pub(super) renamed_symbols: bool,
    pub(super) page_size: Option<u32>,
    pub(super) modified: bool,
    /// Byte ranges of `data` changed since it was read or last written with
    /// [`Writer::write_changes`].
    pub(super) dirty: Vec<Range<usize>>,
    /// The length of the file as it was read or last written with
    /// [`Writer::write_changes`].
    synced_len: usize,
}

impl fmt::Debug for Writer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("len", &self.data.len())
            .field("header", &self.file.header)
            .field("modified", &self.modified)
            .finish_non_exhaustive()
    }
}

impl<'data> Writer<'data> {
    /// Read a file and create a new rewriter.
    ///
    /// Borrowed data is only copied once the file is modified.
    pub fn read(data: impl Into<Cow<'data, [u8]>>) -> Result<Self> {
        let data = data.into();
        let file = ElfFile::parse(&data)?;
        let mut writer = Self {
            dirty: Vec::new(),
            synced_len: data.len(),
            phdrs: file.phdrs.clone(),
            os_abi: file.header.os_abi,
            file,
            data,
            dynamic: None,
            dynstr: None,
            interp: None,
            renamed_symbols: false,
            page_size: None,
            modified: false,
        };
        writer.load_dynamic()?;
        Ok(writer)
    }

    /// Decode the dynamic table and locate the dynamic string table.
    pub(super) fn load_dynamic(&mut self) -> Result<()> {
        self.dynamic = None;
        self.dynstr = None;
        let Some(section) = self.file.dynamic_section() else {
            return Ok(());
        };
        let c = self.file.codec;
        let sh = self.file.shdrs[section];
        let bytes = self.file.section_data(&self.data, &sh)?;
        let mut entries = Vec::new();
        let mut rld_map_rel_target = None;
        for (i, chunk) in bytes.chunks_exact(c.dyn_size()).enumerate() {
            let entry = Dyn::read(c, chunk);
            if entry.tag == DT_NULL {
                break;
            }
            if entry.tag == DT_MIPS_RLD_MAP_REL && self.file.header.e_machine == EM_MIPS {
                let entry_addr = sh.sh_addr + (i * c.dyn_size()) as u64;
                rld_map_rel_target = Some(entry_addr.wrapping_add(entry.val));
            }
            entries.push(entry);
        }

        // `.dynamic` links to its string table; fall back to `DT_STRTAB`.
        let linked = sh.sh_link as usize;
        let strtab = if self
            .file
            .shdrs
            .get(linked)
            .is_some_and(|s| s.sh_type == SHT_STRTAB)
        {
            Some(linked)
        } else {
            entries
                .iter()
                .find(|e| e.tag == DT_STRTAB)
                .and_then(|e| self.file.section_at_address(e.val))
        };
        if let Some(strtab) = strtab {
            let s = self.file.shdrs[strtab];
            slice(&self.data, s.sh_offset, s.sh_size, ".dynstr")?;
            self.dynstr = Some(DynStr {
                section: strtab,
                file_offset: s.sh_offset,
                orig_len: s.sh_size as usize,
                extra: Vec::new(),
            });
        }

        self.dynamic = Some(DynamicTable {
            section,
            entries,
            rld_map_rel_target,
        });
        Ok(())
    }

    pub(super) fn codec(&self) -> Codec {
        self.file.codec
    }

    /// Mutable access to `len` bytes at `at`, which are recorded as changed.
    pub(super) fn bytes_mut(&mut self, at: usize, len: usize) -> &mut [u8] {
        self.dirty.push(at..at + len);
        &mut self.data.to_mut()[at..at + len]
    }

    fn dynamic_mut(&mut self, action: &str) -> Result<&mut DynamicTable> {
        self.dynamic
            .as_mut()
            .ok_or_else(|| ElfError::Modify(format!("No dynamic section found; can't {action}")))
    }

    pub(super) fn dynstr_get(&self, offset: u64) -> Option<&[u8]> {
        self.dynstr.as_ref()?.get(&self.data, offset)
    }

    /// Whether the file has a dynamic section.
    pub fn has_dynamic(&self) -> bool {
        self.dynamic.is_some()
    }

    pub(super) fn entries(&self) -> &[Dyn] {
        self.dynamic.as_ref().map_or(&[], |d| d.entries.as_slice())
    }

    /// The string values of the dynamic entries tagged `tag`.
    fn dynamic_strings(&self, tag: i64) -> impl Iterator<Item = &[u8]> {
        self.entries()
            .iter()
            .filter(move |e| e.tag == tag)
            .filter_map(|e| self.dynstr_get(e.val))
    }

    pub(super) fn dynsym_section(&self) -> Option<usize> {
        self.file.section_by_type(SHT_DYNSYM)
    }

    pub(super) fn dynsym_count(&self) -> usize {
        self.dynsym_section().map_or(0, |i| {
            (self.file.shdrs[i].sh_size / self.codec().sym_size() as u64) as usize
        })
    }

    pub(super) fn dynsym(&self, index: usize) -> Sym {
        let c = self.codec();
        let sh = self.file.shdrs[self.dynsym_section().unwrap()];
        let at = sh.sh_offset as usize + index * c.sym_size();
        Sym::read(c, &self.data[at..at + c.sym_size()])
    }

    pub(super) fn set_dynsym(&mut self, index: usize, sym: Sym) {
        let c = self.codec();
        let sh = self.file.shdrs[self.dynsym_section().unwrap()];
        let at = sh.sh_offset as usize + index * c.sym_size();
        sym.write(c, self.bytes_mut(at, c.sym_size()));
    }

    /// File offset and size of `.gnu.version`, if the file has symbol versions.
    pub(super) fn versym_range(&self) -> Option<(usize, usize)> {
        let i = self.file.section_by_type(SHT_GNU_VERSYM)?;
        let sh = self.file.shdrs[i];
        Some((sh.sh_offset as usize, sh.sh_size as usize))
    }

    /// The `.dynstr` offsets stored in `.gnu.version_r` (`vn_file`, `vna_name`) and
    /// `.gnu.version_d` (`vda_name`), as `(file offset of the field, value, kind)`.
    pub(super) fn version_string_fields(&self) -> Vec<(usize, u32, VersionField)> {
        let c = self.codec();
        let mut fields = Vec::new();
        let data = &self.data[..];
        if let Some(i) = self.file.section_by_type(SHT_GNU_VERNEED) {
            let sh = self.file.shdrs[i];
            let end = (sh.sh_offset + sh.sh_size) as usize;
            let mut at = sh.sh_offset as usize;
            // `sh_info` is the number of entries; the chain also ends at `vn_next == 0`.
            for _ in 0..sh.sh_info.max(1) {
                if at + 16 > end {
                    break;
                }
                fields.push((at + 4, c.u32(data, at + 4), VersionField::NeedFile));
                let count = c.u16(data, at + 2);
                let mut aux = at + c.u32(data, at + 8) as usize;
                for _ in 0..count {
                    if aux + 16 > end {
                        break;
                    }
                    fields.push((aux + 8, c.u32(data, aux + 8), VersionField::NeedName));
                    let next = c.u32(data, aux + 12) as usize;
                    if next == 0 {
                        break;
                    }
                    aux += next;
                }
                let next = c.u32(data, at + 12) as usize;
                if next == 0 {
                    break;
                }
                at += next;
            }
        }
        if let Some(i) = self.file.section_by_type(SHT_GNU_VERDEF) {
            let sh = self.file.shdrs[i];
            let end = (sh.sh_offset + sh.sh_size) as usize;
            let mut at = sh.sh_offset as usize;
            for _ in 0..sh.sh_info.max(1) {
                if at + 20 > end {
                    break;
                }
                let count = c.u16(data, at + 6);
                let mut aux = at + c.u32(data, at + 12) as usize;
                for _ in 0..count {
                    if aux + 8 > end {
                        break;
                    }
                    fields.push((aux, c.u32(data, aux), VersionField::DefName));
                    let next = c.u32(data, aux + 4) as usize;
                    if next == 0 {
                        break;
                    }
                    aux += next;
                }
                let next = c.u32(data, at + 16) as usize;
                if next == 0 {
                    break;
                }
                at += next;
            }
        }
        fields
    }

    /// Every `.dynstr` offset referenced from the file.
    fn string_references(&self) -> Vec<u64> {
        let mut refs: Vec<u64> = self
            .entries()
            .iter()
            .filter(|e| STRING_TAGS.contains(&e.tag))
            .map(|e| e.val)
            .collect();
        for i in 0..self.dynsym_count() {
            refs.push(u64::from(self.dynsym(i).st_name));
        }
        refs.extend(
            self.version_string_fields()
                .into_iter()
                .map(|(_, value, _)| u64::from(value)),
        );
        refs
    }

    /// Store `new` in `.dynstr` to replace the string at `old`, which is referenced
    /// `replaced` times by the references being changed. Returns the offset to use.
    ///
    /// An existing copy of `new` is reused. Otherwise, if nothing else refers to the
    /// old string's bytes and `new` fits, the old string is overwritten in place, so
    /// the table does not grow. Otherwise `new` is appended.
    fn replace_string(&mut self, old: Option<u64>, replaced: usize, new: &[u8]) -> Result<u64> {
        let Some(dynstr) = self.dynstr.as_ref() else {
            return Err(ElfError::Modify(
                "No dynamic string table found; can't store a string".to_string(),
            ));
        };
        if let Some(offset) = dynstr.find(&self.data, new) {
            return Ok(offset);
        }
        if let Some(old) = old.filter(|&o| (o as usize) < dynstr.orig_len) {
            if let Some(old_len) = self.dynstr_get(old).map(<[u8]>::len) {
                let end = old + old_len as u64;
                let refs = self.string_references();
                let exact = refs.iter().filter(|&&r| r == old).count();
                let inside = refs.iter().any(|&r| r > old && r <= end);
                if new.len() <= old_len && exact <= replaced && !inside {
                    let at = self.dynstr.as_ref().unwrap().file_offset as usize + old as usize;
                    let bytes = self.bytes_mut(at, old_len + 1);
                    bytes.fill(0);
                    bytes[..new.len()].copy_from_slice(new);
                    return Ok(old);
                }
            }
        }
        let data = &self.data;
        Ok(self.dynstr.as_mut().unwrap().add(data, new))
    }

    /// Add a dynamic entry, before the terminating `DT_NULL`.
    fn push_dynamic(&mut self, entry: Dyn, action: &str) -> Result<()> {
        self.dynamic_mut(action)?.entries.push(entry);
        Ok(())
    }

    /// Add a `DT_DEBUG` entry to the dynamic section.
    pub fn elf_add_dynamic_debug(&mut self) -> Result<()> {
        let dynamic = self.dynamic_mut("add debug entry")?;
        if dynamic.entries.iter().any(|e| e.tag == DT_DEBUG) {
            return Ok(());
        }
        dynamic.entries.push(Dyn {
            tag: DT_DEBUG,
            val: 0,
        });
        self.modified = true;
        Ok(())
    }

    /// Find the search path the dynamic loader uses: the first `DT_RUNPATH` entry,
    /// or the first `DT_RPATH` entry if there is no `DT_RUNPATH`.
    ///
    /// The loader ignores `DT_RPATH` when `DT_RUNPATH` is present.
    pub fn elf_runpath(&self) -> Option<&[u8]> {
        self.dynamic_strings(DT_RUNPATH)
            .next()
            .or_else(|| self.dynamic_strings(DT_RPATH).next())
    }

    /// Delete any `DT_RUNPATH` or `DT_RPATH` entries in the dynamic section.
    pub fn elf_delete_runpath(&mut self) -> Result<()> {
        let dynamic = self.dynamic_mut("delete runpath")?;
        let before = dynamic.entries.len();
        dynamic
            .entries
            .retain(|e| e.tag != DT_RPATH && e.tag != DT_RUNPATH);
        if dynamic.entries.len() != before {
            self.modified = true;
        }
        Ok(())
    }

    /// Change `DT_RPATH` entries to `DT_RUNPATH` if there is no `DT_RUNPATH`, so
    /// that `LD_LIBRARY_PATH` takes precedence and the path does not apply to
    /// the dependencies' dependencies. When both are present, both are kept.
    fn prefer_runpath(&mut self) {
        let Some(dynamic) = self.dynamic.as_mut() else {
            return;
        };
        if dynamic.entries.iter().any(|e| e.tag == DT_RUNPATH) {
            return;
        }
        for entry in dynamic.entries.iter_mut().filter(|e| e.tag == DT_RPATH) {
            entry.tag = DT_RUNPATH;
        }
    }

    fn rpath_entries(&self) -> Vec<usize> {
        self.entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tag == DT_RPATH || e.tag == DT_RUNPATH)
            .map(|(i, _)| i)
            .collect()
    }

    fn set_entry_strings(&mut self, indices: &[usize], value: &[u8]) -> Result<()> {
        let offsets: HashSet<u64> = indices.iter().map(|&i| self.entries()[i].val).collect();
        let old = if offsets.len() == 1 {
            offsets.into_iter().next()
        } else {
            None
        };
        let offset = self.replace_string(old, indices.len(), value)?;
        let dynamic = self.dynamic.as_mut().unwrap();
        for &i in indices {
            dynamic.entries[i].val = offset;
        }
        Ok(())
    }

    /// Set the path for any `DT_RUNPATH` or `DT_RPATH` entry in the dynamic section.
    ///
    /// A `DT_RPATH` becomes a `DT_RUNPATH` unless the file has both.
    pub fn elf_set_runpath(&mut self, runpath: Vec<u8>) -> Result<()> {
        self.dynamic_mut("set runpath")?;
        self.prefer_runpath();
        let indices = self.rpath_entries();
        if indices.is_empty() {
            let offset = self.replace_string(None, 0, &runpath)?;
            self.push_dynamic(
                Dyn {
                    tag: DT_RUNPATH,
                    val: offset,
                },
                "set runpath",
            )?;
        } else {
            self.set_entry_strings(&indices, &runpath)?;
        }
        self.modified = true;
        Ok(())
    }

    /// Add additional paths to any `DT_RUNPATH` or `DT_RPATH` entry in the dynamic section.
    ///
    /// A `DT_RPATH` becomes a `DT_RUNPATH` unless the file has both.
    pub fn elf_add_runpath(&mut self, runpaths: &[Vec<u8>]) -> Result<()> {
        self.dynamic_mut("add runpath")?;
        self.prefer_runpath();
        let indices = self.rpath_entries();
        if indices.is_empty() {
            let value = runpaths.join(&b':');
            let offset = self.replace_string(None, 0, &value)?;
            self.push_dynamic(
                Dyn {
                    tag: DT_RUNPATH,
                    val: offset,
                },
                "add runpath",
            )?;
        } else {
            for i in indices {
                let mut value = self
                    .dynstr_get(self.entries()[i].val)
                    .unwrap_or_default()
                    .to_vec();
                for path in runpaths {
                    if !value.is_empty() {
                        value.push(b':');
                    }
                    value.extend_from_slice(path);
                }
                self.set_entry_strings(&[i], &value)?;
            }
        }
        self.modified = true;
        Ok(())
    }

    fn retag(&mut self, from: i64, to: i64, action: &str) -> Result<()> {
        let dynamic = self.dynamic_mut(action)?;
        let mut changed = false;
        for entry in dynamic.entries.iter_mut().filter(|e| e.tag == from) {
            entry.tag = to;
            changed = true;
        }
        if changed {
            self.modified = true;
        }
        Ok(())
    }

    /// Change any `DT_RPATH` entry in the dynamic section to `DT_RUNPATH`.
    pub fn elf_use_runpath(&mut self) -> Result<()> {
        self.retag(DT_RPATH, DT_RUNPATH, "change runpath")
    }

    /// Change any `DT_RUNPATH` entry in the dynamic section to `DT_RPATH`.
    pub fn elf_use_rpath(&mut self) -> Result<()> {
        self.retag(DT_RUNPATH, DT_RPATH, "change rpath")
    }

    /// Find the `DT_NEEDED` entries in the dynamic section.
    pub fn elf_needed(&self) -> impl Iterator<Item = &[u8]> {
        self.dynamic_strings(DT_NEEDED)
    }

    /// Delete `DT_NEEDED` entries from the dynamic section.
    pub fn elf_delete_needed(&mut self, names: &HashSet<Vec<u8>>) -> Result<()> {
        self.dynamic_mut("delete needed library")?;
        let keep: Vec<bool> = self
            .entries()
            .iter()
            .map(|e| {
                e.tag != DT_NEEDED || !self.dynstr_get(e.val).is_some_and(|n| names.contains(n))
            })
            .collect();
        let dynamic = self.dynamic.as_mut().unwrap();
        let before = dynamic.entries.len();
        let mut keep = keep.into_iter();
        dynamic.entries.retain(|_| keep.next().unwrap());
        if dynamic.entries.len() != before {
            self.modified = true;
        }
        Ok(())
    }

    /// Replace `DT_NEEDED` entries in the dynamic section.
    ///
    /// Version requirements (`.gnu.version_r`) on a replaced library are moved to
    /// the new name, since the dynamic loader matches them by library name.
    pub fn elf_replace_needed(&mut self, names: &HashMap<Vec<u8>, Vec<u8>>) -> Result<()> {
        self.dynamic_mut("replace needed library")?;
        for i in 0..self.entries().len() {
            let entry = self.entries()[i];
            if entry.tag != DT_NEEDED {
                continue;
            }
            let Some(new) = self
                .dynstr_get(entry.val)
                .and_then(|n| names.get(n))
                .cloned()
            else {
                continue;
            };
            self.set_entry_strings(&[i], &new)?;
            self.modified = true;
        }

        let c = self.codec();
        for (at, value, kind) in self.version_string_fields() {
            if kind != VersionField::NeedFile {
                continue;
            }
            let Some(new) = self
                .dynstr_get(u64::from(value))
                .and_then(|n| names.get(n))
                .cloned()
            else {
                continue;
            };
            let offset = self.replace_string(Some(u64::from(value)), 1, &new)?;
            c.put_u32(self.bytes_mut(at, 4), 0, offset as u32);
            self.modified = true;
        }
        Ok(())
    }

    /// Add `DT_NEEDED` entries to the start of the dynamic section.
    ///
    /// This does not add a `DT_NEEDED` entry if the library is already listed.
    pub fn elf_add_needed(&mut self, names: &[Vec<u8>]) -> Result<()> {
        self.dynamic_mut("add needed library")?;
        let found: HashSet<Vec<u8>> = self.elf_needed().map(<[u8]>::to_vec).collect();
        let mut added = Vec::new();
        for name in names.iter().filter(|n| !found.contains(n.as_slice())) {
            let offset = self.replace_string(None, 0, name)?;
            added.push(Dyn {
                tag: DT_NEEDED,
                val: offset,
            });
        }
        if !added.is_empty() {
            let dynamic = self.dynamic.as_mut().unwrap();
            dynamic.entries.splice(0..0, added);
            self.modified = true;
        }
        Ok(())
    }

    /// The soname is only read and written for shared objects.
    fn is_shared_object(&self) -> bool {
        self.file.header.e_type == ET_DYN
    }

    /// Find the `DT_SONAME` entry in the dynamic section.
    ///
    /// Returns `None` for files that are not shared objects.
    pub fn elf_soname(&self) -> Option<&[u8]> {
        if !self.is_shared_object() {
            return None;
        }
        self.dynamic_strings(DT_SONAME).next()
    }

    /// Set the `DT_SONAME` entry in the dynamic section.
    ///
    /// Only shared objects (`ET_DYN`, which includes position-independent
    /// executables) have a soname: nothing reads it in an `ET_EXEC` file, which
    /// cannot be linked against. So this fails for other files.
    pub fn elf_set_soname(&mut self, soname: Vec<u8>) -> Result<()> {
        if !self.is_shared_object() {
            return Err(ElfError::Modify(
                "not a shared object (ET_DYN); only shared objects have a soname".into(),
            ));
        }
        self.dynamic_mut("set soname")?;
        let indices: Vec<usize> = self
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tag == DT_SONAME)
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            let offset = self.replace_string(None, 0, &soname)?;
            self.push_dynamic(
                Dyn {
                    tag: DT_SONAME,
                    val: offset,
                },
                "set soname",
            )?;
        } else {
            self.set_entry_strings(&indices, &soname)?;
        }
        self.modified = true;
        Ok(())
    }

    /// Find the interpreter path in the `PT_INTERP` segment.
    ///
    /// The NUL terminator stored in the section is not included.
    pub fn elf_interpreter(&self) -> Option<&[u8]> {
        let data = match &self.interp {
            Some(interp) => interp.as_slice(),
            None => {
                let sh = self.file.shdrs[self.file.interp_section()?];
                self.file.section_data(&self.data, &sh).ok()?
            }
        };
        Some(data.split(|&b| b == 0).next().unwrap_or(data))
    }

    /// Set the interpreter path in the `PT_INTERP` segment.
    ///
    /// The null terminator is automatically added if needed.
    pub fn elf_set_interpreter(&mut self, mut interpreter: Vec<u8>) -> Result<()> {
        if self.file.interp_section().is_none() {
            return Err(ElfError::Modify(
                "No interp section found; can't set interpreter".to_string(),
            ));
        }
        if interpreter.last() != Some(&0) {
            interpreter.push(0);
        }
        self.interp = Some(interpreter);
        self.modified = true;
        Ok(())
    }

    /// Set the EI_OSABI to some ABI
    pub fn elf_set_osabi(&mut self, abi_name: &str) -> Result<()> {
        // Case-insensitive, surrounding whitespace ignored.
        let abi_name = abi_name.trim().to_lowercase();

        let new_abi = match abi_name.as_str() {
            "sysv" | "system v" | "system-v" => ELFOSABI_SYSV,
            "hpux" | "hp-ux" => ELFOSABI_HPUX,
            "netbsd" => ELFOSABI_NETBSD,
            "linux" | "gnu" => ELFOSABI_LINUX,
            "hurd" | "gnu-hurd" | "gnu hurd" => ELFOSABI_HURD,
            "solaris" => ELFOSABI_SOLARIS,
            "aix" => ELFOSABI_AIX,
            "irix" => ELFOSABI_IRIX,
            "freebsd" => ELFOSABI_FREEBSD,
            "tru64" => ELFOSABI_TRU64,
            "modesto" => ELFOSABI_MODESTO,
            "openbsd" => ELFOSABI_OPENBSD,
            "openvms" => ELFOSABI_OPENVMS,
            "nsk" => ELFOSABI_NSK,
            "aros" => ELFOSABI_AROS,
            "fenixos" => ELFOSABI_FENIXOS,
            "cloudabi" => ELFOSABI_CLOUDABI,
            _ => return Err(ElfError::Modify(format!("Unknown ABI {abi_name}"))),
        };

        self.os_abi = new_abi;
        self.modified = true;
        Ok(())
    }

    /// The `EI_OSABI` byte of the file header.
    pub fn os_abi(&self) -> u8 {
        self.os_abi
    }

    /// Remove from the DT_RUNPATH or DT_RPATH all directories that do not contain a library referenced by DT_NEEDED.
    ///
    /// A `DT_RPATH` becomes a `DT_RUNPATH` unless the file has both. A file
    /// without either is left unchanged.
    pub fn elf_shrink_rpath(&mut self, allowed_rpath_prefixes: Vec<String>) -> Result<()> {
        self.dynamic_mut("shrink rpath")?;
        let e_machine = self.file.header.e_machine;
        let needed_libraries: HashSet<Vec<u8>> = self.elf_needed().map(<[u8]>::to_vec).collect();
        let indices = self.rpath_entries();
        // A file without a search path is left as it is.
        if indices.is_empty() {
            return Ok(());
        }
        self.prefer_runpath();
        for i in indices {
            let rpath = self.dynstr_get(self.entries()[i].val).unwrap_or_default();
            let rpath_str = String::from_utf8_lossy(rpath).into_owned();
            let mut rpath_vec: Vec<&str> = rpath_str.split(':').collect();

            // Check if each directory contains a library referenced by DT_NEEDED
            rpath_vec.retain(|&dir| {
                let dir_path = Path::new(dir);

                // Keep non-absolute paths (e.g., "$ORIGIN")
                if !dir.starts_with('/') {
                    return true;
                };

                // Check allowed prefixes
                if !allowed_rpath_prefixes.is_empty()
                    && !allowed_rpath_prefixes
                        .iter()
                        .any(|prefix| dir.starts_with(prefix))
                {
                    eprintln!("Removing {dir} from RPATH due to non-allowed prefix");
                    return false;
                }

                for lib_needed in needed_libraries.iter() {
                    let Ok(possible_library) = String::from_utf8(lib_needed.to_vec()) else {
                        continue;
                    };
                    let Ok(file_content) = fs::read(dir_path.join(possible_library)) else {
                        continue;
                    };
                    if elf_machine(&file_content) == Some(e_machine) {
                        return true;
                    }
                }
                eprintln!("Removing directory {dir}");
                false
            });

            eprintln!("New rpath: {rpath_vec:?}");

            let new_rpath = rpath_vec.join(":");
            self.set_entry_strings(&[i], new_rpath.as_bytes())?;
        }
        self.modified = true;
        Ok(())
    }

    /// Disable the default library search paths
    pub fn elf_no_default_lib(&mut self) -> Result<()> {
        let dynamic = self.dynamic_mut("set no-default-lib")?;
        match dynamic.entries.iter_mut().find(|e| e.tag == DT_FLAGS_1) {
            Some(entry) => entry.val |= DF_1_NODEFLIB,
            None => dynamic.entries.push(Dyn {
                tag: DT_FLAGS_1,
                val: DF_1_NODEFLIB,
            }),
        }
        self.modified = true;
        Ok(())
    }

    /// Clear the symbol version information for a symbol
    ///
    /// Fails if the file has no symbol versions (`.gnu.version`).
    pub fn elf_clear_symbol_version(&mut self, symbol: &str) -> Result<()> {
        let Some((versym_offset, versym_size)) = self.versym_range() else {
            return Err(ElfError::MissingSection(".gnu.version"));
        };
        let c = self.codec();
        let mut found = false;
        for i in 0..self.dynsym_count().min(versym_size / 2) {
            let name = self.dynstr_get(u64::from(self.dynsym(i).st_name));
            if name != Some(symbol.as_bytes()) {
                continue;
            }
            c.put_u16(self.bytes_mut(versym_offset + i * 2, 2), 0, VER_NDX_GLOBAL);
            found = true;
        }
        if found {
            self.modified = true;
        }
        Ok(())
    }

    fn gnu_stack_mut(&mut self) -> &mut ProgramHeader {
        if let Some(i) = self.phdrs.iter().position(|p| p.p_type == PT_GNU_STACK) {
            return &mut self.phdrs[i];
        }
        let stack = ProgramHeader {
            p_type: PT_GNU_STACK,
            p_flags: PF_R | PF_W | PF_X,
            p_offset: 0,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: 0,
            p_memsz: 0,
            p_align: 0x10,
        };
        // Reuse an unused entry rather than growing the program header table.
        match self.phdrs.iter().position(|p| p.p_type == PT_NULL) {
            Some(i) => {
                self.phdrs[i] = stack;
                &mut self.phdrs[i]
            }
            None => {
                self.phdrs.push(stack);
                self.phdrs.last_mut().unwrap()
            }
        }
    }

    /// Remove the execute permission from the stack (`PT_GNU_STACK`).
    pub fn elf_clear_exec_stack(&mut self) -> Result<()> {
        self.gnu_stack_mut().p_flags &= !PF_X;
        self.modified = true;
        Ok(())
    }

    /// Make the stack executable (`PT_GNU_STACK`).
    pub fn elf_set_exec_stack(&mut self) -> Result<()> {
        self.gnu_stack_mut().p_flags |= PF_X;
        self.modified = true;
        Ok(())
    }

    /// Return the flags of the PT_GNU_STACK segment
    pub fn elf_gnu_exec_stack(&self) -> Option<u32> {
        self.phdrs
            .iter()
            .find(|p| p.p_type == PT_GNU_STACK)
            .map(|p| p.p_flags)
    }

    /// Rename dynamic symbols.
    ///
    /// The `names` map is from old names to new names. The symbol hash tables
    /// are rebuilt when the file is written.
    pub fn elf_rename_dynamic_symbols(&mut self, names: &HashMap<Vec<u8>, Vec<u8>>) -> Result<()> {
        for i in 0..self.dynsym_count() {
            let mut sym = self.dynsym(i);
            let Some(new) = self
                .dynstr_get(u64::from(sym.st_name))
                .and_then(|n| names.get(n))
                .cloned()
            else {
                continue;
            };
            let offset = self.replace_string(Some(u64::from(sym.st_name)), 1, &new)?;
            sym.st_name = u32::try_from(offset)
                .map_err(|_| ElfError::Modify("dynamic string table too large".into()))?;
            self.set_dynsym(i, sym);
            self.renamed_symbols = true;
            self.modified = true;
        }
        Ok(())
    }

    /// Write the file to an output stream.
    pub fn write<W: io::Write>(mut self, mut w: W) -> Result<()> {
        self.commit()?;
        w.write_all(&self.data).map_err(ElfError::Io)?;
        w.flush().map_err(ElfError::Io)
    }

    /// Write the patched ELF file to a path.
    ///
    /// The whole file is laid out in memory before `path` is opened, so an error
    /// while laying out the file leaves `path` untouched. `path` is then
    /// overwritten in place, which keeps its inode, permissions, ownership and
    /// hard links. The writer stays usable afterwards.
    pub fn write_to_path(&mut self, path: &Path) -> Result<()> {
        self.commit()?;
        fs::write(path, &self.data).map_err(ElfError::Io)
    }

    /// Write only the bytes that changed to `file`, which must hold the bytes
    /// this writer was read from, or wrote with its last `write_changes`.
    ///
    /// This is what patching a file in place needs: for a large file with a
    /// small change it writes a few pages instead of the whole file. As with
    /// [`Writer::write_to_path`], everything is laid out in memory before
    /// `file` is written to. `file` must be open for writing.
    pub fn write_changes(&mut self, file: &mut fs::File) -> Result<()> {
        self.commit()?;
        let on_disk = file.metadata().map_err(ElfError::Io)?.len();
        if on_disk != self.synced_len as u64 {
            return Err(ElfError::Modify(format!(
                "the file is {on_disk} bytes long, but {} bytes were read; it changed since",
                self.synced_len
            )));
        }
        // Writing the bytes between nearby ranges costs less than another write.
        for range in merge_ranges(&self.dirty, 4096) {
            file.seek(SeekFrom::Start(range.start as u64))
                .map_err(ElfError::Io)?;
            file.write_all(&self.data[range]).map_err(ElfError::Io)?;
        }
        file.flush().map_err(ElfError::Io)?;
        self.dirty.clear();
        self.synced_len = self.data.len();
        Ok(())
    }

    /// The bytes of the file, with all modifications applied.
    pub fn to_bytes(&mut self) -> Result<&[u8]> {
        self.commit()?;
        Ok(&self.data)
    }

    /// Set the page size for ELF file segment alignment.
    pub fn elf_set_page_size(&mut self, page_size: u32) -> Result<()> {
        // Validate page size (must be power of 2 and >= 1024)
        if !page_size.is_power_of_two() || page_size < 1024 {
            return Err(ElfError::Modify(format!(
                "Page size must be a power of 2 and >= 1024, got {page_size}",
            )));
        }

        self.page_size = Some(page_size);
        self.modified = true;

        // Update program header alignments
        for segment in &mut self.phdrs {
            if segment.p_type == PT_LOAD {
                segment.p_align = u64::from(page_size);
            }
        }

        Ok(())
    }

    /// Get the current page size used for segment alignment.
    pub fn elf_get_page_size(&self) -> u32 {
        if let Some(page_size) = self.page_size {
            return page_size;
        }

        // The largest page size each architecture supports.
        match self.file.header.e_machine {
            // 65536
            EM_ALPHA | EM_IA_64 | EM_MIPS | EM_PPC | EM_PPC64 | EM_AARCH64 | EM_TILEGX
            | EM_LOONGARCH => 0x10000,
            // 8192
            EM_SPARC | EM_SPARCV9 => 0x2000,
            // 4096 (default)
            _ => 0x1000,
        }
    }
}

/// Sort and merge byte ranges, also those less than `gap` bytes apart.
fn merge_ranges(ranges: &[Range<usize>], gap: usize) -> Vec<Range<usize>> {
    let mut sorted: Vec<Range<usize>> = ranges.iter().filter(|r| !r.is_empty()).cloned().collect();
    sorted.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last) if range.start <= last.end + gap => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Which `.dynstr` reference of the version sections a field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VersionField {
    /// `vn_file`: the library a version requirement is on.
    NeedFile,
    /// `vna_name`: a required version.
    NeedName,
    /// `vda_name`: a defined version.
    DefName,
}

/// The `e_machine` of an ELF file, in its own byte order.
fn elf_machine(data: &[u8]) -> Option<u16> {
    if data.len() < 20 || data[..4] != ELFMAG {
        return None;
    }
    let raw = [data[18], data[19]];
    match data[EI_DATA] {
        ELFDATA2LSB => Some(u16::from_le_bytes(raw)),
        ELFDATA2MSB => Some(u16::from_be_bytes(raw)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture, little- and big-endian.
    fn fixtures() -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for root in ["../../tests/data/elf", "../../tests/data/elf-big-endian"] {
            for arch in fs::read_dir(root).unwrap() {
                for kind in fs::read_dir(arch.unwrap().path()).unwrap() {
                    for file in fs::read_dir(kind.unwrap().path()).unwrap() {
                        files.push(file.unwrap().path());
                    }
                }
            }
        }
        files.sort();
        assert!(files.len() >= 8);
        files
    }

    fn assert_recorded(path: &std::path::Path, orig: &[u8], w: &Writer) {
        for (i, (a, b)) in orig.iter().zip(w.data.iter()).enumerate() {
            if a != b {
                assert!(
                    w.dirty.iter().any(|r| r.contains(&i)),
                    "{}: byte {i:#x} changed but is not recorded",
                    path.display()
                );
            }
        }
    }

    /// Every byte that differs from the original file lies in a recorded dirty
    /// range, so `write_changes` writes all of them.
    #[test]
    fn dirty_ranges_cover_every_change() {
        for path in fixtures() {
            let orig = fs::read(&path).unwrap();
            let mut w = Writer::read(orig.clone()).unwrap();
            // A defined symbol to rename, which may reorder the symbols.
            let defined = (1..w.dynsym_count())
                .map(|i| w.dynsym(i))
                .find(|s| s.st_shndx != SHN_UNDEF && s.st_name != 0)
                .and_then(|s| w.dynstr_get(u64::from(s.st_name)))
                .map(<[u8]>::to_vec);
            let mut renames = HashMap::from([(b"setlocale".to_vec(), b"setlocale2".to_vec())]);
            if let Some(name) = defined {
                let mut new = name.clone();
                new.extend_from_slice(b"_renamed");
                renames.insert(name, new);
            }
            w.elf_rename_dynamic_symbols(&renames).unwrap();
            // liblld.so has no symbol versions to clear.
            let cleared = w.elf_clear_symbol_version("setlocale2");
            assert!(
                matches!(
                    cleared,
                    Ok(()) | Err(ElfError::MissingSection(".gnu.version"))
                ),
                "{cleared:?}"
            );
            w.elf_set_runpath(b"/a".to_vec()).unwrap();
            if w.file.header.e_type == ET_DYN {
                w.elf_set_soname(b"s".to_vec()).unwrap();
            }
            w.elf_add_needed(&[b"libextra.so".to_vec()]).unwrap();
            w.elf_replace_needed(&HashMap::from([(
                b"libc.so.6".to_vec(),
                b"libc.so.7".to_vec(),
            )]))
            .unwrap();
            let _ = w.elf_set_interpreter(vec![b'i'; 200]);
            w.elf_set_exec_stack().unwrap();
            w.elf_set_osabi("freebsd").unwrap();
            w.commit().unwrap();
            assert_recorded(&path, &orig, &w);
            if w.data.len() > orig.len() {
                assert!(w
                    .dirty
                    .iter()
                    .any(|r| r.start <= orig.len() && r.end == w.data.len()));
            }

            // Edits that fit where they are.
            let mut w = Writer::read(orig.clone()).unwrap();
            if w.elf_set_interpreter(b"/i".to_vec()).is_ok() {
                w.commit().unwrap();
                assert_recorded(&path, &orig, &w);
            }

            // A dynamic table that has to move, with the symbols defined in it.
            let mut w = Writer::read(orig.clone()).unwrap();
            let many: Vec<Vec<u8>> = (0..64)
                .map(|i| format!("libmany{i}.so").into_bytes())
                .collect();
            w.elf_add_needed(&many).unwrap();
            w.commit().unwrap();
            assert_recorded(&path, &orig, &w);
        }
    }

    #[test]
    fn ranges_are_sorted_and_merged() {
        assert_eq!(merge_ranges(&[], 4096), vec![]);
        assert_eq!(
            merge_ranges(
                &[
                    10_000..10_010,
                    0..4,
                    2..8,
                    5..5,
                    20_000..20_001,
                    10_005..14_000
                ],
                4096
            ),
            vec![0..8, 10_000..14_000, 20_000..20_001]
        );
        // Ranges less than a page apart are written together.
        assert_eq!(merge_ranges(&[0..4, 4000..4004], 4096), vec![0..4004]);
        assert_eq!(merge_ranges(&[0..4, 4..8, 9..10], 0), vec![0..8, 9..10]);
    }
}
