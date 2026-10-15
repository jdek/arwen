//! Writing the edits of a [`Writer`] back into the file.
//!
//! Every edited section that still fits where it is (its old size, or the free
//! space up to the next section) is written in place. Sections that no longer
//! fit are moved into a single new `PT_LOAD` segment appended to the file, or
//! appended to a segment at the end of the file that an earlier run added.
//!
//! Adding a segment needs a program header. When the program header table has
//! no room to grow, the movable sections right after it (interpreter, notes,
//! hash and symbol tables, version and relocation tables) are moved out as one
//! block. Only if that is impossible is the table itself moved into the new
//! segment, at an address chosen so that `AT_PHDR` stays correct even on
//! kernels that derive it from the first `PT_LOAD`.
//!
//! Limitation: when `.dynamic` has to move, code that locates its own dynamic
//! section without `PT_DYNAMIC` (through `_DYNAMIC` or `GOT[0]`, as dynamic
//! loaders and self-relocating objects do) still sees the old copy. `.dynamic`
//! is therefore only moved when its spare slots and the free space after it
//! are exhausted.

use super::codec::{reloc_sym, reloc_with_sym, Dyn, ProgramHeader, SectionHeader, Sym};
use super::consts::*;
use super::file::ElfFile;
use super::hash::{
    gnu_hash, gnu_symbol_order, rebuild_gnu, rebuild_sysv, sysv_hash, GnuHashHeader,
};
use super::writer::Writer;
use super::{ElfError, Result};

/// Print layout decisions to stderr.
const TRACE: bool = false;

macro_rules! trace {
    ($($arg:tt)*) => {
        if TRACE {
            eprintln!("arwen-elf layout: {}", format_args!($($arg)*));
        }
    };
}

/// Extra `DT_NULL` slots given to a moved dynamic table, so that later
/// additions fit in place: at least this many, or half the entries.
const SPARE_DYNAMIC_SLOTS: usize = 4;

/// Dynamic tags holding the address of a section.
const POINTER_TAGS: [i64; 11] = [
    DT_STRTAB,
    DT_SYMTAB,
    DT_HASH,
    DT_GNU_HASH,
    DT_VERSYM,
    DT_VERNEED,
    DT_VERDEF,
    DT_RELA,
    DT_REL,
    DT_JMPREL,
    DT_RELR,
];

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        return value;
    }
    value.div_ceil(align) * align
}

/// The smallest value `>= value` that is congruent to `target` modulo `align`.
fn align_congruent(value: u64, target: u64, align: u64) -> u64 {
    if align <= 1 {
        return value;
    }
    value + (target % align + align - value % align) % align
}

/// Where a section ends up.
#[derive(Debug, Clone, Copy)]
struct Relocation {
    offset_delta: i128,
    addr_delta: i128,
}

fn shift(value: u64, delta: i128) -> u64 {
    (i128::from(value) + delta) as u64
}

/// How the program header table gets the room for more entries.
enum PhdrRoom {
    /// The space after the table is free.
    Fits,
    /// The sections in this file range move out of the way, as one block.
    Block(u64, u64),
    /// The table moves into the new segment.
    Move,
}

/// A section whose contents change size: its new size and, unless it is the
/// dynamic table, which is serialized last, its new contents.
struct Edit {
    section: usize,
    size: u64,
    contents: Option<Vec<u8>>,
}

impl Writer<'_> {
    /// Apply all edits to the file bytes and re-read the result.
    pub(super) fn commit(&mut self) -> Result<()> {
        if !self.modified {
            return Ok(());
        }
        if self.renamed_symbols {
            self.rebuild_symbol_hashes()?;
        }
        self.lay_out()?;

        // Re-read the written file so the writer reflects it and stays usable.
        self.file = ElfFile::parse(&self.data)?;
        self.phdrs = self.file.phdrs.clone();
        self.os_abi = self.file.header.os_abi;
        self.interp = None;
        self.renamed_symbols = false;
        self.modified = false;
        self.load_dynamic()
    }

    fn vaddr_to_offset(&self, addr: u64) -> Option<u64> {
        self.file
            .phdrs
            .iter()
            .find(|p| p.p_type == PT_LOAD && p.p_vaddr <= addr && addr < p.p_vaddr + p.p_filesz)
            .map(|p| addr - p.p_vaddr + p.p_offset)
    }

    fn dynamic_value(&self, tag: i64) -> Option<u64> {
        self.entries().iter().find(|e| e.tag == tag).map(|e| e.val)
    }

    /// Rebuild `.gnu.hash` and `.hash` after symbols were renamed.
    ///
    /// `.gnu.hash` requires the hashed symbols to be grouped by bucket, so the
    /// dynamic symbols may have to be reordered, together with `.gnu.version`
    /// and the symbol indices of the dynamic relocations.
    fn rebuild_symbol_hashes(&mut self) -> Result<()> {
        let c = self.codec();
        self.ensure_dynsym()?;
        for sh_type in [SHT_GNU_HASH, SHT_HASH, SHT_GNU_VERSYM] {
            if let Some(i) = self.file.section_by_type(sh_type) {
                self.ensure_section(i)?;
            }
        }
        let count = self.dynsym_count();
        let gnu = self.file.section_by_type(SHT_GNU_HASH);
        let sysv = self.file.section_by_type(SHT_HASH);
        // Each name is hashed once for each table.
        let mut gnu_hashes = Vec::with_capacity(if gnu.is_some() { count } else { 0 });
        let mut sysv_hashes = Vec::with_capacity(if sysv.is_some() { count } else { 0 });
        for i in 0..count {
            let name = self
                .dynstr_get(u64::from(self.dynsym(i).st_name))
                .unwrap_or_default();
            if gnu.is_some() {
                gnu_hashes.push(gnu_hash(name));
            }
            if sysv.is_some() {
                sysv_hashes.push(sysv_hash(name));
            }
        }

        if let Some(i) = gnu {
            let sh = self.file.shdrs[i];
            let table = self.file.section_data(&self.data, &sh)?;
            let header = GnuHashHeader::read(c, table)?;
            let end = header.hashed_end(c, table.len(), count)?;
            // Symbols past the hashed range cannot be found through the table,
            // so they must not be defined.
            if (end..count).any(|i| self.dynsym(i).st_shndx != SHN_UNDEF) {
                return Err(ElfError::Malformed(
                    ".gnu.hash does not cover every defined dynamic symbol".into(),
                ));
            }
            let mut order = gnu_symbol_order(header, &gnu_hashes[..end]);
            order.extend(end..count);
            if order.iter().enumerate().any(|(new, &old)| new != old) {
                trace!("reordering {} dynamic symbols for .gnu.hash", count);
                self.permute_dynamic_symbols(&order)?;
                let permute = |hashes: &[u32]| -> Vec<u32> {
                    if hashes.is_empty() {
                        return Vec::new();
                    }
                    order.iter().map(|&from| hashes[from]).collect()
                };
                gnu_hashes = permute(&gnu_hashes);
                sysv_hashes = permute(&sysv_hashes);
            }
            let at = sh.sh_offset as usize;
            rebuild_gnu(
                c,
                self.bytes_mut(at, sh.sh_size as usize),
                &gnu_hashes[..end],
            )?;
        }

        if let Some(i) = sysv {
            let sh = self.file.shdrs[i];
            let entry_size = if sh.sh_entsize == 8 { 8 } else { 4 };
            let at = sh.sh_offset as usize;
            rebuild_sysv(
                c,
                entry_size,
                self.bytes_mut(at, sh.sh_size as usize),
                &sysv_hashes,
            )?;
        }
        Ok(())
    }

    /// Reorder the dynamic symbols: `order[new] = old`.
    fn permute_dynamic_symbols(&mut self, order: &[usize]) -> Result<()> {
        let c = self.codec();
        if self.file.header.e_machine == EM_MIPS {
            // The MIPS GOT layout depends on the order of the dynamic symbols.
            return Err(ElfError::Unsupported(
                "renaming dynamic symbols that requires reordering them is not supported on MIPS"
                    .into(),
            ));
        }
        for tag in [DT_SYMINFO, DT_ANDROID_REL, DT_ANDROID_RELA] {
            if self.dynamic_value(tag).is_some() {
                return Err(ElfError::Unsupported(format!(
                    "renaming dynamic symbols in files with dynamic tag {tag:#x} is not supported"
                )));
            }
        }

        let mut new_index = vec![0u64; order.len()];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new as u64;
        }

        let dynsym = self.file.shdrs[self.dynsym_section().unwrap()];
        let size = c.sym_size();
        let base = dynsym.sh_offset as usize;
        let old: Vec<u8> = self.data[base..base + order.len() * size].to_vec();
        let data = self.bytes_mut(base, order.len() * size);
        for (new, &from) in order.iter().enumerate() {
            data[new * size..(new + 1) * size]
                .copy_from_slice(&old[from * size..(from + 1) * size]);
        }

        // Versions, parallel to the symbols.
        if let Some((at, len)) = self.versym_range() {
            if len / 2 >= order.len() {
                let old: Vec<u8> = self.data[at..at + order.len() * 2].to_vec();
                let data = self.bytes_mut(at, order.len() * 2);
                for (new, &from) in order.iter().enumerate() {
                    data[new * 2..new * 2 + 2].copy_from_slice(&old[from * 2..from * 2 + 2]);
                }
            }
        }

        // Relocations, found the way the dynamic loader finds them, plus any
        // relocation section linked to `.dynsym`. Each entry is rewritten once.
        let word = c.word_size() as u64;
        let mut tables: Vec<(u64, u64, u64)> = Vec::new(); // (offset, size, entry size)
        let mut add_range = |w: &Self, addr: Option<u64>, size: Option<u64>, ent: u64| {
            if let (Some(addr), Some(size)) = (addr, size) {
                if let Some(offset) = w.vaddr_to_offset(addr) {
                    tables.push((offset, size, ent));
                }
            }
        };
        let rela_ent = self.dynamic_value(DT_RELAENT).unwrap_or(3 * word);
        let rel_ent = self.dynamic_value(DT_RELENT).unwrap_or(2 * word);
        add_range(
            self,
            self.dynamic_value(DT_RELA),
            self.dynamic_value(DT_RELASZ),
            rela_ent,
        );
        add_range(
            self,
            self.dynamic_value(DT_REL),
            self.dynamic_value(DT_RELSZ),
            rel_ent,
        );
        let plt_ent = if self.dynamic_value(DT_PLTREL) == Some(DT_RELA as u64) {
            rela_ent
        } else {
            rel_ent
        };
        add_range(
            self,
            self.dynamic_value(DT_JMPREL),
            self.dynamic_value(DT_PLTRELSZ),
            plt_ent,
        );
        let dynsym_index = self.dynsym_section().unwrap() as u32;
        for sh in &self.file.shdrs {
            if (sh.sh_type == SHT_RELA || sh.sh_type == SHT_REL) && sh.sh_link == dynsym_index {
                let ent = if sh.sh_type == SHT_RELA {
                    rela_ent
                } else {
                    rel_ent
                };
                tables.push((sh.sh_offset, sh.sh_size, ent));
            }
        }
        let ranges: Vec<_> = tables
            .iter()
            .map(|&(offset, size, _)| offset..offset + size)
            .collect();
        self.ensure_all(&ranges)?;
        // A table may also be found through the dynamic section; rewrite each
        // entry once.
        tables.retain(|&(_, size, ent)| ent != 0 && size >= ent);
        tables.sort_unstable();
        tables.dedup();
        let mut entries: Vec<u64> = tables
            .iter()
            .flat_map(|&(offset, size, ent)| (0..size / ent).map(move |i| offset + i * ent))
            .collect();
        entries.sort_unstable();
        entries.dedup();
        let data = self.data.to_mut();
        for at in entries {
            let info_at = (at + word) as usize;
            if info_at + word as usize > data.len() {
                return Err(ElfError::Malformed("relocation outside the file".into()));
            }
            let info = c.word(data, info_at);
            let sym = reloc_sym(c, info);
            if sym == 0 {
                continue;
            }
            let Some(&new) = new_index.get(sym as usize) else {
                return Err(ElfError::Malformed(format!(
                    "relocation refers to dynamic symbol {sym}, which does not exist"
                )));
            };
            c.put_word(data, info_at, reloc_with_sym(c, info, new));
        }
        // One dirty range per table rather than per entry.
        for &(offset, size, ent) in &tables {
            let end = (offset + size / ent * ent).min(data.len() as u64);
            self.dirty.push(offset as usize..end as usize);
        }
        Ok(())
    }

    /// The bytes available to section `index` without moving it, and the
    /// `PT_LOAD` to extend if the section ends at the end of the file.
    fn capacity(&self, index: usize) -> (u64, Option<usize>) {
        let sh = self.file.shdrs[index];
        let (off, addr) = (sh.sh_offset, sh.sh_addr);
        let Some((load_index, load)) = self.file.phdrs.iter().enumerate().find(|(_, p)| {
            p.p_type == PT_LOAD
                && p.p_offset <= off
                && off + sh.sh_size <= p.file_end()
                && p.p_vaddr <= addr
                && addr + sh.sh_size <= p.mem_end()
        }) else {
            return (sh.sh_size, None);
        };

        if self.is_trailing_load(load_index) && off + sh.sh_size == load.file_end() {
            return (u64::MAX, Some(load_index));
        }

        let mut limit_off = load.file_end();
        let mut limit_addr = load.mem_end();
        for (j, other) in self.file.shdrs.iter().enumerate() {
            if j == index {
                continue;
            }
            if other.has_file_data()
                && other.sh_offset >= off
                && !(other.sh_offset == off && other.sh_size == 0)
            {
                limit_off = limit_off.min(other.sh_offset);
            }
            let tls_bss = other.sh_type == SHT_NOBITS && other.sh_flags & 0x400 != 0;
            if other.is_alloc()
                && !tls_bss
                && other.sh_addr >= addr
                && !(other.sh_addr == addr && other.sh_size == 0)
            {
                limit_addr = limit_addr.min(other.sh_addr);
            }
        }
        let h = &self.file.header;
        for table in [h.e_phoff, h.e_shoff] {
            if table >= off && table != 0 {
                limit_off = limit_off.min(table);
            }
        }
        let available = (limit_off - off).min(limit_addr.saturating_sub(addr));
        (available.max(sh.sh_size), None)
    }

    /// Whether `PT_LOAD` `index` is the last segment both in memory and in the
    /// file, with no uninitialized part, so it can be extended at the end.
    fn is_trailing_load(&self, index: usize) -> bool {
        let load = self.file.phdrs[index];
        let highest = self
            .file
            .phdrs
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .all(|p| p.p_vaddr <= load.p_vaddr);
        highest
            && load.p_filesz == load.p_memsz
            && load.file_end() == self.data.len() as u64
            && self.file.header.e_shoff < load.p_offset
    }

    /// Whether a section may be moved into the new segment: every reference to
    /// it is known and updated (dynamic pointer tags, segments, section headers).
    fn is_movable(&self, index: usize) -> bool {
        let sh = self.file.shdrs[index];
        if !sh.is_alloc() || !sh.has_file_data() || sh.sh_flags & SHF_WRITE != 0 {
            return false;
        }
        let dynstr = self.dynstr.as_ref().map(|d| d.section);
        matches!(
            sh.sh_type,
            SHT_NOTE
                | SHT_HASH
                | SHT_GNU_HASH
                | SHT_DYNSYM
                | SHT_GNU_VERSYM
                | SHT_GNU_VERDEF
                | SHT_GNU_VERNEED
                | SHT_REL
                | SHT_RELA
                | SHT_RELR
        ) || Some(index) == dynstr
            || Some(index) == self.file.interp_section()
    }

    /// Sections and the section header table overlapping the file range
    /// `[start, end)` or the address range `[addr, addr + end - start)`.
    fn obstacles(&self, start: u64, end: u64, addr: u64) -> (Vec<usize>, bool) {
        let addr_end = addr + (end - start);
        let mut sections = Vec::new();
        for (i, sh) in self.file.shdrs.iter().enumerate() {
            let in_file = sh.has_file_data()
                && sh.sh_offset < end
                && (sh.sh_offset + sh.sh_size > start
                    || (sh.sh_size == 0 && sh.sh_offset >= start));
            let tls_bss = sh.sh_type == SHT_NOBITS && sh.sh_flags & 0x400 != 0;
            let in_memory = sh.is_alloc()
                && !tls_bss
                && sh.sh_addr < addr_end
                && (sh.sh_addr + sh.sh_size > addr || (sh.sh_size == 0 && sh.sh_addr >= addr));
            if in_file || in_memory {
                sections.push(i);
            }
        }
        let h = &self.file.header;
        let shdr_end =
            h.e_shoff + (self.file.shdrs.len() * self.codec().section_header_size()) as u64;
        let header_table = h.e_shoff < end && shdr_end > start;
        (sections, header_table)
    }

    /// Whether the program header table can grow from `old_phnum` to `phnum`
    /// entries where it is, and what has to move to make that possible.
    fn phdr_room(
        &self,
        phdr_load: Option<ProgramHeader>,
        old_phnum: usize,
        phnum: usize,
        moved: &[usize],
    ) -> PhdrRoom {
        let esz = self.codec().program_header_size() as u64;
        let phoff = self.file.header.e_phoff;
        let start = phoff + old_phnum as u64 * esz;
        let end = phoff + phnum as u64 * esz;
        let Some(load) = phdr_load else {
            return PhdrRoom::Move;
        };
        let addr = load.p_vaddr + (start - load.p_offset);
        if end > load.file_end() || addr + (end - start) > load.mem_end() {
            return PhdrRoom::Move;
        }
        let (mut sections, header_table) = self.obstacles(start, end, addr);
        // Sections that move anyway free their old place.
        sections.retain(|i| !moved.contains(i));
        if header_table {
            return PhdrRoom::Move;
        }
        if sections.is_empty() {
            return PhdrRoom::Fits;
        }
        if !sections.iter().all(|&i| self.is_movable(i)) {
            return PhdrRoom::Move;
        }

        // Move the obstructing sections out as one block, closed over any
        // section or segment that would otherwise be split.
        let shdrs = &self.file.shdrs;
        let mut lo = sections.iter().map(|&i| shdrs[i].sh_offset).min().unwrap();
        let mut hi = sections
            .iter()
            .map(|&i| shdrs[i].sh_offset + shdrs[i].sh_size)
            .max()
            .unwrap();
        loop {
            let mut changed = false;
            for (i, sh) in shdrs.iter().enumerate() {
                if !sh.has_file_data() || sh.sh_offset >= hi || sh.sh_offset + sh.sh_size <= lo {
                    continue;
                }
                if !self.is_movable(i) {
                    return PhdrRoom::Move;
                }
                if sh.sh_offset < lo || sh.sh_offset + sh.sh_size > hi {
                    lo = lo.min(sh.sh_offset);
                    hi = hi.max(sh.sh_offset + sh.sh_size);
                    changed = true;
                }
            }
            for p in &self.file.phdrs {
                if p.p_type == PT_LOAD || p.p_type == PT_PHDR || p.p_filesz == 0 {
                    continue;
                }
                if p.p_offset < hi && p.file_end() > lo && (p.p_offset < lo || p.file_end() > hi) {
                    lo = lo.min(p.p_offset);
                    hi = hi.max(p.file_end());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        if lo < start || hi > load.file_end() {
            return PhdrRoom::Move;
        }
        PhdrRoom::Block(lo, hi)
    }

    fn lay_out(&mut self) -> Result<()> {
        let c = self.codec();
        let page = u64::from(self.elf_get_page_size());
        let data_len = self.data.len() as u64;
        let old_shdrs = self.file.shdrs.clone();
        let interp_section = self.file.interp_section();

        // 1. Sections whose size changes, and whether they fit where they are.
        let mut edits: Vec<Edit> = Vec::new();
        if let Some(d) = &self.dynstr {
            if !d.extra.is_empty() {
                edits.push(Edit {
                    section: d.section,
                    size: d.len() as u64,
                    contents: Some(d.contents(&self.data)),
                });
            }
        }
        if let (Some(interp), Some(section)) = (&self.interp, interp_section) {
            edits.push(Edit {
                section,
                size: interp.len() as u64,
                contents: Some(interp.clone()),
            });
        }
        if let Some(d) = &self.dynamic {
            let needed = ((d.entries.len() + 1) * c.dyn_size()) as u64;
            edits.push(Edit {
                section: d.section,
                size: needed.max(old_shdrs[d.section].sh_size),
                contents: None,
            });
        }

        let mut moved: Vec<usize> = Vec::new();
        // Sections at the end of a trailing segment that grow in place:
        // (section, segment, new end offset).
        let mut grow_at_end: Vec<(usize, usize, u64)> = Vec::new();
        for edit in &mut edits {
            let old = old_shdrs[edit.section];
            let (available, trailing) = self.capacity(edit.section);
            if edit.size <= available {
                if let Some(load) = trailing {
                    grow_at_end.push((edit.section, load, old.sh_offset + edit.size));
                }
                trace!(
                    "section {} stays at {:#x} ({:#x} -> {:#x} bytes)",
                    edit.section,
                    old.sh_offset,
                    old.sh_size,
                    edit.size
                );
                continue;
            }
            if Some(edit.section) == self.dynamic.as_ref().map(|d| d.section) {
                let entries = self.entries().len();
                edit.size += (SPARE_DYNAMIC_SLOTS.max(entries / 2) * c.dyn_size()) as u64;
            }
            trace!(
                "section {} moves ({:#x} bytes, {:#x} available)",
                edit.section,
                edit.size,
                available
            );
            moved.push(edit.section);
        }
        let dynamic_moves = self
            .dynamic
            .as_ref()
            .is_some_and(|d| moved.contains(&d.section));

        // 2. Program headers: an existing trailing segment can take the moved
        //    sections, otherwise a new PT_LOAD is needed.
        let trailing = (0..self.file.phdrs.len())
            .find(|&i| self.file.phdrs[i].p_type == PT_LOAD && self.is_trailing_load(i));
        let old_phnum = self.file.phdrs.len();
        let esz = c.program_header_size() as u64;
        let phoff = self.file.header.e_phoff;
        let phdr_load = self
            .file
            .phdrs
            .iter()
            .find(|p| p.p_type == PT_LOAD && p.p_offset <= phoff && phoff < p.file_end())
            .copied();

        let mut block: Option<(u64, u64)> = None; // old file range moved as one unit
        let mut move_phdrs = false;
        let mut need_segment = !moved.is_empty();
        // At most two rounds: making room may itself require the new segment.
        loop {
            let new_load = need_segment && trailing.is_none();
            let phnum = self.phdrs.len() + usize::from(new_load);
            if phnum <= old_phnum {
                break;
            }
            match self.phdr_room(phdr_load, old_phnum, phnum, &moved) {
                PhdrRoom::Fits => break,
                PhdrRoom::Block(lo, hi) => {
                    trace!(
                        "moving block {:#x}..{:#x} to make room for program headers",
                        lo,
                        hi
                    );
                    block = Some((lo, hi));
                    if need_segment {
                        break;
                    }
                    need_segment = true;
                }
                PhdrRoom::Move => {
                    trace!("program headers cannot grow in place, moving them");
                    move_phdrs = true;
                    block = None;
                    need_segment = true;
                    break;
                }
            }
        }

        // Edited sections inside the block keep their place in it only if they
        // do not grow; the free space after them is not part of the block.
        if let Some((lo, hi)) = block {
            for edit in &edits {
                let old = old_shdrs[edit.section];
                if old.sh_offset >= lo
                    && old.sh_offset < hi
                    && edit.size > old.sh_size
                    && !moved.contains(&edit.section)
                {
                    moved.push(edit.section);
                }
            }
        }
        let dynamic_moves = dynamic_moves
            || self
                .dynamic
                .as_ref()
                .is_some_and(|d| moved.contains(&d.section));
        let use_trailing = need_segment && trailing.is_some() && !move_phdrs;
        let new_load = need_segment && !use_trailing;

        // The trailing segment grows in place for sections at its end; new
        // contents are placed after that.
        let extend_load: Option<(usize, u64)> = grow_at_end
            .iter()
            .filter(|(section, _, _)| !moved.contains(section))
            .map(|&(_, load, end)| (load, end))
            .max_by_key(|&(_, end)| end);
        let file_end = data_len.max(extend_load.map_or(0, |(_, end)| end));

        // 3. Place the new segment's contents.
        let phnum = self.phdrs.len() + usize::from(new_load);
        // Where each section that moves goes, by section index.
        let mut relocated: Vec<Option<Relocation>> = vec![None; old_shdrs.len()];
        let mut new_shdrs = old_shdrs.clone();
        let mut new_phoff = phoff;
        let mut new_phaddr = phdr_load.map(|l| l.p_vaddr + (phoff - l.p_offset));
        let mut segment: Option<ProgramHeader> = None;
        let mut segment_end = file_end;
        let mut block_delta: Option<Relocation> = None;

        if need_segment {
            let (base_offset, base_vaddr) = if use_trailing {
                let load = self.file.phdrs[trailing.unwrap()];
                (file_end, load.p_vaddr + (file_end - load.p_offset))
            } else {
                let mut max_end = self
                    .phdrs
                    .iter()
                    .filter(|p| p.p_type == PT_LOAD)
                    .map(ProgramHeader::mem_end)
                    .max()
                    .unwrap_or(0);
                if let Some((load, end)) = extend_load {
                    let load = self.file.phdrs[load];
                    max_end = max_end.max(load.p_vaddr + (end - load.p_offset));
                }
                let first = self.file.phdrs.iter().find(|p| p.p_type == PT_LOAD);
                match first {
                    Some(first)
                        if move_phdrs
                            && self.file.segment(PT_INTERP).is_some()
                            && first.p_vaddr.wrapping_sub(first.p_offset) % page == 0 =>
                    {
                        // Keep `vaddr - offset` equal to that of the first PT_LOAD, so
                        // that AT_PHDR computed from it points at the moved table on
                        // kernels that derive it that way.
                        let delta = first.p_vaddr.wrapping_sub(first.p_offset);
                        let offset = file_end.max(align_up(max_end, page).wrapping_sub(delta));
                        (offset, offset.wrapping_add(delta))
                    }
                    _ => (file_end, align_up(max_end, page) + file_end % page),
                }
            };
            let to_addr = |offset: u64| base_vaddr + (offset - base_offset);
            let mut cursor = base_offset;

            if move_phdrs {
                cursor = align_up(cursor, c.word_size() as u64);
                new_phoff = cursor;
                new_phaddr = Some(to_addr(cursor));
                cursor += phnum as u64 * esz;
                trace!("program headers move to {:#x}", new_phoff);
            }
            if let Some((lo, hi)) = block {
                let align = old_shdrs
                    .iter()
                    .filter(|sh| sh.has_file_data() && sh.sh_offset >= lo && sh.sh_offset < hi)
                    .map(|sh| sh.sh_addralign.max(1))
                    .max()
                    .unwrap_or(1)
                    .min(page);
                cursor = align_congruent(cursor, lo, align);
                let block_addr = phdr_load
                    .map(|l| l.p_vaddr + (lo - l.p_offset))
                    .unwrap_or(lo);
                let delta = Relocation {
                    offset_delta: i128::from(cursor) - i128::from(lo),
                    addr_delta: i128::from(to_addr(cursor)) - i128::from(block_addr),
                };
                for (i, sh) in old_shdrs.iter().enumerate() {
                    if sh.has_file_data()
                        && sh.sh_offset >= lo
                        && sh.sh_offset < hi
                        && sh.sh_size > 0
                    {
                        relocated[i] = Some(delta);
                        new_shdrs[i].sh_offset = shift(sh.sh_offset, delta.offset_delta);
                        new_shdrs[i].sh_addr = shift(sh.sh_addr, delta.addr_delta);
                    }
                }
                block_delta = Some(delta);
                cursor += hi - lo;
            }
            // `.dynstr` goes last: it is what later edits grow most, and at the
            // end of the segment it can grow in place next time.
            let dynstr = self.dynstr.as_ref().map(|d| d.section);
            moved.sort_by_key(|&i| (Some(i) == dynstr, old_shdrs[i].sh_addr));
            for &i in &moved {
                let edit = edits.iter().find(|e| e.section == i).unwrap();
                let align = old_shdrs[i].sh_addralign.max(1).min(page);
                cursor = align_up(cursor, align);
                relocated[i] = Some(Relocation {
                    offset_delta: i128::from(cursor) - i128::from(old_shdrs[i].sh_offset),
                    addr_delta: i128::from(to_addr(cursor)) - i128::from(old_shdrs[i].sh_addr),
                });
                new_shdrs[i].sh_offset = cursor;
                new_shdrs[i].sh_addr = to_addr(cursor);
                trace!(
                    "section {} placed at {:#x} / {:#x}",
                    i,
                    cursor,
                    to_addr(cursor)
                );
                cursor += edit.size;
            }
            segment_end = cursor;
            if to_addr(segment_end) > c.max_address() || segment_end > c.max_address() {
                return Err(ElfError::Modify(
                    "the patched file does not fit in the 32-bit address space".into(),
                ));
            }
            if !use_trailing {
                let flags = PF_R | if dynamic_moves { PF_W } else { 0 };
                segment = Some(ProgramHeader {
                    p_type: PT_LOAD,
                    p_flags: flags,
                    p_offset: base_offset,
                    p_vaddr: base_vaddr,
                    p_paddr: base_vaddr,
                    p_filesz: segment_end - base_offset,
                    p_memsz: segment_end - base_offset,
                    p_align: page,
                });
            }
        }
        for edit in &edits {
            new_shdrs[edit.section].sh_size = edit.size;
        }

        // 4. Adjust symbols defined in sections that moved.
        let symbol_tables: Vec<SectionHeader> = old_shdrs
            .iter()
            .filter(|sh| sh.sh_type == SHT_DYNSYM || sh.sh_type == SHT_SYMTAB)
            .copied()
            .collect();
        if relocated.iter().any(Option::is_some) {
            let ranges: Vec<_> = symbol_tables
                .iter()
                .map(|sh| sh.sh_offset..sh.sh_offset + sh.sh_size)
                .collect();
            self.ensure_all(&ranges)?;
            let data = self.data.to_mut();
            for table in symbol_tables {
                let size = c.sym_size();
                let base = table.sh_offset as usize;
                let mut changed = false;
                for k in 0..(table.sh_size as usize / size) {
                    let at = base + k * size;
                    let mut sym = Sym::read(c, &data[at..at + size]);
                    if sym.st_shndx == SHN_UNDEF || sym.st_shndx >= SHN_LORESERVE {
                        continue;
                    }
                    let Some(rel) = relocated.get(usize::from(sym.st_shndx)).copied().flatten()
                    else {
                        continue;
                    };
                    let sh = old_shdrs[usize::from(sym.st_shndx)];
                    if sym.st_value < sh.sh_addr || sym.st_value > sh.sh_addr + sh.sh_size {
                        continue;
                    }
                    sym.st_value = shift(sym.st_value, rel.addr_delta);
                    sym.write(c, &mut data[at..at + size]);
                    changed = true;
                }
                if changed {
                    // One range per table rather than one per symbol.
                    let len = table.sh_size as usize / size * size;
                    self.dirty.push(base..base + len);
                }
            }
        }

        // 5. The dynamic table, with pointers to moved sections updated.
        let mut dynamic_bytes = None;
        if let Some(d) = &self.dynamic {
            let mut entries = d.entries.clone();
            for entry in &mut entries {
                if POINTER_TAGS.contains(&entry.tag) {
                    if let Some(i) = old_shdrs
                        .iter()
                        .position(|sh| sh.is_alloc() && sh.sh_addr == entry.val && sh.sh_size > 0)
                    {
                        if let Some(rel) = relocated[i] {
                            entry.val = shift(entry.val, rel.addr_delta);
                        }
                    }
                }
                if entry.tag == DT_STRSZ {
                    if let Some(dynstr) = &self.dynstr {
                        entry.val = dynstr.len() as u64;
                    }
                }
            }
            let sh = new_shdrs[d.section];
            if let Some(target) = d.rld_map_rel_target {
                if let Some(k) = entries.iter().position(|e| e.tag == DT_MIPS_RLD_MAP_REL) {
                    let entry_addr = sh.sh_addr + (k * c.dyn_size()) as u64;
                    entries[k].val = target.wrapping_sub(entry_addr);
                }
            }
            let mut bytes = vec![0u8; sh.sh_size as usize];
            for (k, entry) in entries.iter().enumerate() {
                entry.write(c, &mut bytes[k * c.dyn_size()..(k + 1) * c.dyn_size()]);
            }
            Dyn {
                tag: DT_NULL,
                val: 0,
            }
            .write(c, &mut bytes[entries.len() * c.dyn_size()..]);
            dynamic_bytes = Some((sh.sh_offset as usize, bytes));
        }

        // 6. Program headers.
        let mut phdrs = self.phdrs.clone();
        let new_phaddr = new_phaddr.unwrap_or(0);
        for p in &mut phdrs {
            match p.p_type {
                PT_PHDR if move_phdrs || phnum != old_phnum => {
                    p.p_offset = new_phoff;
                    p.p_vaddr = new_phaddr;
                    p.p_paddr = new_phaddr;
                    p.p_filesz = phnum as u64 * esz;
                    p.p_memsz = p.p_filesz;
                }
                PT_INTERP if interp_section.is_some() => {
                    let sh = new_shdrs[interp_section.unwrap()];
                    if self.interp.is_some() || relocated[interp_section.unwrap()].is_some() {
                        p.p_offset = sh.sh_offset;
                        p.p_vaddr = sh.sh_addr;
                        p.p_paddr = sh.sh_addr;
                        p.p_filesz = sh.sh_size;
                        p.p_memsz = sh.sh_size;
                    }
                }
                PT_DYNAMIC if self.dynamic.is_some() => {
                    let sh = new_shdrs[self.dynamic.as_ref().unwrap().section];
                    p.p_offset = sh.sh_offset;
                    p.p_vaddr = sh.sh_addr;
                    p.p_paddr = sh.sh_addr;
                    p.p_filesz = sh.sh_size;
                    p.p_memsz = sh.sh_size;
                }
                PT_LOAD => {}
                _ => {
                    if let (Some((lo, hi)), Some(delta)) = (block, block_delta) {
                        if p.p_filesz > 0 && p.p_offset >= lo && p.file_end() <= hi {
                            p.p_offset = shift(p.p_offset, delta.offset_delta);
                            p.p_vaddr = shift(p.p_vaddr, delta.addr_delta);
                            p.p_paddr = shift(p.p_paddr, delta.addr_delta);
                        }
                    }
                }
            }
        }
        if let Some((load, end)) = extend_load {
            let p = &mut phdrs[load];
            p.p_filesz = p.p_filesz.max(end - p.p_offset);
            p.p_memsz = p.p_filesz;
        }
        if use_trailing {
            let p = &mut phdrs[trailing.unwrap()];
            p.p_filesz = segment_end - p.p_offset;
            p.p_memsz = p.p_filesz;
            if dynamic_moves {
                p.p_flags |= PF_W;
            }
        }
        if let Some(segment) = segment {
            // Keep PT_LOAD entries sorted by address: the new one is the highest.
            let after = phdrs
                .iter()
                .rposition(|p| p.p_type == PT_LOAD)
                .map_or(phdrs.len(), |i| i + 1);
            phdrs.insert(after, segment);
        }
        if phdrs.len() >= 0xffff {
            return Err(ElfError::Modify("too many program headers".into()));
        }

        // 7. Write everything.
        if let Some((lo, hi)) = block {
            self.ensure(lo..hi)?;
        }
        let total = segment_end.max(file_end) as usize;
        let data = self.data.to_mut();
        let dirty = &mut self.dirty;
        if total > data.len() {
            dirty.push(data.len()..total);
        }
        data.resize(total, 0);
        // Contents of sections that stay, at their place.
        for edit in &edits {
            if moved.contains(&edit.section) {
                continue;
            }
            if let Some(contents) = &edit.contents {
                let old = old_shdrs[edit.section];
                let at = old.sh_offset as usize;
                let len = (old.sh_size as usize).max(contents.len());
                data[at..at + len].fill(0);
                data[at..at + contents.len()].copy_from_slice(contents);
                dirty.push(at..at + len);
            }
        }
        if let (Some((lo, hi)), Some(delta)) = (block, block_delta) {
            let to = shift(lo, delta.offset_delta) as usize;
            data.copy_within(lo as usize..hi as usize, to);
            dirty.push(to..to + (hi - lo) as usize);
        }
        // Clear what moved sections leave behind, so that nothing reading the
        // old place finds stale tables. Not when `.dynamic` moves: its old copy
        // stays in use (see the limitation above) and points at them.
        if !dynamic_moves {
            let mut vacated: Vec<(u64, u64)> = moved
                .iter()
                .map(|&i| old_shdrs[i])
                .filter(SectionHeader::has_file_data)
                .map(|sh| (sh.sh_offset, sh.sh_offset + sh.sh_size))
                .chain(block)
                .collect();
            for sh in new_shdrs
                .iter()
                .filter(|sh| sh.has_file_data() && sh.sh_size > 0)
            {
                let (lo, hi) = (sh.sh_offset, sh.sh_offset + sh.sh_size);
                vacated = vacated
                    .into_iter()
                    .flat_map(|(a, b)| [(a, b.min(lo)), (a.max(hi), b)])
                    .filter(|(a, b)| a < b)
                    .collect();
            }
            for (lo, hi) in vacated {
                let range = lo as usize..(hi as usize).min(data_len as usize);
                if !range.is_empty() {
                    data[range.clone()].fill(0);
                    dirty.push(range);
                }
            }
        }
        for &i in &moved {
            let edit = edits.iter().find(|e| e.section == i).unwrap();
            if let Some(contents) = &edit.contents {
                let at = new_shdrs[i].sh_offset as usize;
                data[at..at + contents.len()].copy_from_slice(contents);
                dirty.push(at..at + contents.len());
            }
        }
        if let Some((at, bytes)) = dynamic_bytes {
            data[at..at + bytes.len()].copy_from_slice(&bytes);
            dirty.push(at..at + bytes.len());
        }
        for (k, p) in phdrs.iter().enumerate() {
            let at = new_phoff as usize + k * esz as usize;
            p.write(c, &mut data[at..at + esz as usize]);
        }
        dirty.push(new_phoff as usize..new_phoff as usize + phdrs.len() * esz as usize);
        let mut header = self.file.header.clone();
        header.os_abi = self.os_abi;
        header.e_phoff = new_phoff;
        header.e_phnum = phdrs.len() as u16;
        header.write(c, data);
        dirty.push(0..c.file_header_size());
        let shsz = c.section_header_size();
        for (k, sh) in new_shdrs.iter().enumerate() {
            let at = header.e_shoff as usize + k * shsz;
            sh.write(c, &mut data[at..at + shsz]);
        }
        let shoff = header.e_shoff as usize;
        dirty.push(shoff..shoff + new_shdrs.len() * shsz);
        Ok(())
    }
}
