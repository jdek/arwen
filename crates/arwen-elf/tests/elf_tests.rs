use arwen_elf::ElfContainer;
use goblin::elf::Elf;
use rstest::rstest;
use std::path::PathBuf;

/// This test checks if the rpath of a Elf file can be set.
#[rstest]
fn test_rpath_set(#[files("../../tests/data/elf/x64/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();

    elf_container
        .set_runpath("path_graf_path_graf_path_graf_path_graf")
        .unwrap();

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_elf = Elf::parse(&changed_elf_data).unwrap();

    insta::assert_debug_snapshot!(changed_elf.runpaths);
    insta::assert_snapshot!(changed_elf_data.len());
}

/// This test checks if the rpath of an ELF file can be removed.
#[rstest]
fn test_rpath_remove(#[files("../../tests/data/elf/x64/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();

    elf_container.remove_runpath().unwrap();

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_elf = Elf::parse(&changed_elf_data).unwrap();

    insta::assert_debug_snapshot!(changed_elf.runpaths);
    insta::assert_snapshot!(changed_elf_data.len());
}

/// This test verify that a rpath can be added to an ELF file.
#[rstest]
fn test_rpath_add(#[files("../../tests/data/elf/x64/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();

    elf_container
        .add_runpath("abababababababababababaabbababababababababab")
        .unwrap();

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_macho = Elf::parse(&changed_elf_data).unwrap();

    insta::assert_debug_snapshot!(changed_macho.runpaths);
    insta::assert_snapshot!(changed_elf_data.len());
}

/// This test verify that force_rpath works.
#[rstest]
fn test_force_rpath(#[files("../../tests/data/elf/x64/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();

    elf_container.force_rpath().unwrap();

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_macho = Elf::parse(&changed_elf_data).unwrap();

    insta::assert_debug_snapshot!(changed_macho.rpaths);
    insta::assert_snapshot!(changed_elf_data.len());
}

/// This test verifies that page size can be set for ELF files
#[rstest]
fn test_set_page_size(#[files("../../tests/data/elf/x64/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();

    // Test getting default page size first
    let default_page_size = elf_container.get_page_size();
    assert!(default_page_size > 0);

    // Set a custom page size (8KB)
    elf_container.set_page_size(8192).unwrap();

    // Verify the page size was set correctly
    assert_eq!(elf_container.get_page_size(), 8192);

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_elf = Elf::parse(&changed_elf_data).unwrap();

    // Verify PT_LOAD segments have correct alignment
    for ph in &changed_elf.program_headers {
        if ph.p_type == goblin::elf::program_header::PT_LOAD {
            assert_eq!(ph.p_align, 8192);
        }
    }

    insta::assert_debug_snapshot!(changed_elf.program_headers);
    insta::assert_snapshot!(changed_elf_data.len());
}

/// `PT_INTERP` must cover exactly the new interpreter and its NUL terminator,
/// whether or not the `.interp` section had to move. The kernel rejects the
/// file if the last byte of the segment is not NUL.
#[rstest]
fn test_set_interpreter_updates_pt_interp(
    #[files("../../tests/data/elf/*/exec/*")] bin_path: PathBuf,
    #[values(
        "/ld.so",
        "/lib64/test-ld.so",
        "/a/much/longer/interpreter/path/that/needs/more/room/ld.so"
    )]
    interpreter: &str,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container.set_interpreter(interpreter).unwrap();

    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed_elf = Elf::parse(&changed_elf_data).unwrap();
    assert_eq!(changed_elf.interpreter, Some(interpreter));

    let interp = changed_elf
        .program_headers
        .iter()
        .find(|ph| ph.p_type == goblin::elf::program_header::PT_INTERP)
        .unwrap();
    assert_eq!(interp.p_filesz, interpreter.len() as u64 + 1);
    let end = (interp.p_offset + interp.p_filesz) as usize;
    assert_eq!(changed_elf_data[end - 1], 0);
}

struct FailingWriter;

impl std::io::Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk full"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Errors from the underlying writer must be reported, including those that
/// only surface when the internal buffer is flushed at the end.
#[rstest]
fn test_write_error_is_reported(#[files("../../tests/data/elf/*/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container.set_runpath("/x").unwrap();

    let result = elf_container.write(FailingWriter);
    assert!(
        matches!(result, Err(arwen_elf::ElfError::Io(_))),
        "{result:?}"
    );
}

/// Modifications must not shrink segments that linkers pad on purpose.
///
/// `PT_GNU_RELRO` ends on a page boundary so that the dynamic loader, which
/// rounds the end down, also protects the last page. Only `PT_INTERP` and
/// `PT_DYNAMIC` track their section exactly.
#[rstest]
fn test_padded_segments_are_not_shrunk(
    #[files("../../tests/data/elf/*/exec/*")]
    #[files("../../tests/data/elf/*/lib/*")]
    bin_path: PathBuf,
    #[values("set_rpath", "remove_rpath", "add_needed", "remove_needed")] operation: &str,
) {
    use goblin::elf::program_header::{PT_DYNAMIC, PT_INTERP, PT_LOAD};

    let data_bytes = std::fs::read(&bin_path).unwrap();
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    match operation {
        "set_rpath" => elf_container.set_runpath("/x").unwrap(),
        "remove_rpath" => elf_container.remove_runpath().unwrap(),
        "add_needed" => elf_container.add_needed(vec!["libextra.so"]).unwrap(),
        "remove_needed" => elf_container.remove_needed(vec!["libc.so.6"]).unwrap(),
        _ => unreachable!(),
    }
    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let original = Elf::parse(&data_bytes).unwrap();
    let changed = Elf::parse(&changed_elf_data).unwrap();
    for old in &original.program_headers {
        if matches!(old.p_type, PT_LOAD | PT_INTERP | PT_DYNAMIC) {
            continue;
        }
        let Some(new) = changed.program_headers.iter().find(|ph| {
            ph.p_type == old.p_type
                && ph.p_vaddr <= old.p_vaddr
                && old.p_vaddr < ph.p_vaddr + ph.p_memsz.max(1)
        }) else {
            // Segments without size (e.g. PT_GNU_STACK) are matched by type alone.
            assert!(
                changed
                    .program_headers
                    .iter()
                    .any(|ph| ph.p_type == old.p_type),
                "segment {old:?} disappeared"
            );
            continue;
        };
        assert!(
            new.p_vaddr + new.p_memsz >= old.p_vaddr + old.p_memsz,
            "segment shrank:\nold: {old:?}\nnew: {new:?}"
        );
        assert!(
            new.p_offset + new.p_filesz >= old.p_offset + old.p_filesz,
            "segment shrank:\nold: {old:?}\nnew: {new:?}"
        );
    }
}

/// A container stays usable after writing: a second modification and write
/// applies on top of the first.
#[rstest]
fn test_write_twice(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container
        .set_runpath("/first/runpath/that/is/longer/than/the/original/one")
        .unwrap();
    elf_container.write_to_path(&out).unwrap();
    elf_container.add_needed(vec!["libsecond.so"]).unwrap();
    elf_container.write_to_path(&out).unwrap();

    let written = std::fs::read(&out).unwrap();
    let elf = Elf::parse(&written).unwrap();
    assert_eq!(
        elf.runpaths
            .iter()
            .chain(&elf.rpaths)
            .copied()
            .collect::<Vec<_>>(),
        ["/first/runpath/that/is/longer/than/the/original/one"]
    );
    assert_eq!(elf.libraries[0], "libsecond.so");

    // Reading the written file back gives the same view.
    let reread = ElfContainer::parse(&written).unwrap();
    assert_eq!(
        reread.get_rpath(),
        ["/first/runpath/that/is/longer/than/the/original/one"]
    );
}

/// Relocatable objects and core dumps have no dynamic section to patch.
#[rstest]
#[case::relocatable(1)]
#[case::core(4)]
fn test_non_loadable_files_are_rejected(#[case] e_type: u8) {
    let mut data = std::fs::read("../../tests/data/elf/x64/lib/libfoo.so").unwrap();
    // e_type, little-endian.
    data[16] = e_type;
    data[17] = 0;
    let Err(err) = ElfContainer::parse(&data) else {
        panic!("parsing succeeded");
    };
    assert!(
        matches!(err, arwen_elf::ElfError::Unsupported(_)),
        "unexpected error: {err}"
    );
}

/// Patching the same file over and over does not keep copying everything: once
/// the growing tables sit at the end of the file, they grow in place.
#[rstest]
fn test_repeated_patching_growth_is_bounded(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    const ROUNDS: usize = 16;
    let mut data = std::fs::read(&bin_path).unwrap();
    let mut sizes = vec![data.len()];
    for round in 0..ROUNDS {
        let mut elf_container = ElfContainer::parse(&data).unwrap();
        let runpath = format!("/round/{round}/{}", "r".repeat(100));
        elf_container.add_runpath(&runpath).unwrap();
        elf_container
            .add_needed(vec![format!("libround{round}.so")])
            .unwrap();
        let mut out = Vec::new();
        elf_container.write(&mut out).unwrap();
        data = out;
        sizes.push(data.len());
    }
    // A round appends the runpath (one copy of the old value plus the new path,
    // under 4 KiB here) and the library name. Only the rounds where the dynamic
    // table runs out of spare entries may copy more.
    let big_rounds = sizes[2..]
        .windows(2)
        .filter(|pair| pair[1] - pair[0] >= 4096)
        .count();
    assert!(big_rounds <= 1, "too many rounds copied tables: {sizes:?}");
}

fn sysv_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xf000_0000;
        h ^= g >> 24;
        h &= !g;
    }
    h
}

fn gnu_hash(name: &[u8]) -> u32 {
    name.iter().fold(5381u32, |h, &c| {
        h.wrapping_mul(33).wrapping_add(u32::from(c))
    })
}

/// Look `name` up in the dynamic symbol table the way the dynamic loader does,
/// through `.gnu.hash` and through `.hash`. Returns the symbol index found by
/// each table that the file has.
fn hash_lookups(elf: &Elf, data: &[u8], name: &str) -> Vec<Option<usize>> {
    let u32_at = |at: usize| {
        let b: [u8; 4] = data[at..at + 4].try_into().unwrap();
        if elf.little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let word = if elf.is_64 { 8 } else { 4 };
    let word_at = |at: usize| -> u64 {
        if elf.is_64 {
            let b: [u8; 8] = data[at..at + 8].try_into().unwrap();
            if elf.little_endian {
                u64::from_le_bytes(b)
            } else {
                u64::from_be_bytes(b)
            }
        } else {
            u64::from(u32_at(at))
        }
    };
    let sym_name = |i: usize| elf.dynstrtab.get_at(elf.dynsyms.get(i).unwrap().st_name);
    let mut found = Vec::new();

    if let Some(sh) = elf
        .section_headers
        .iter()
        .find(|sh| sh.sh_type == goblin::elf::section_header::SHT_GNU_HASH)
    {
        let base = sh.sh_offset as usize;
        let (nbuckets, symoffset, bloom_size, shift) = (
            u32_at(base),
            u32_at(base + 4),
            u32_at(base + 8),
            u32_at(base + 12),
        );
        let bits = word as u32 * 8;
        let h = gnu_hash(name.as_bytes());
        let bloom = word_at(base + 16 + ((h / bits) % bloom_size) as usize * word);
        let mask = (1u64 << (h % bits)) | (1u64 << ((h >> shift) % bits));
        let buckets = base + 16 + bloom_size as usize * word;
        let chains = buckets + nbuckets as usize * 4;
        let mut result = None;
        if bloom & mask == mask {
            let mut i = u32_at(buckets + (h % nbuckets) as usize * 4) as usize;
            if i != 0 {
                loop {
                    let chain = u32_at(chains + (i - symoffset as usize) * 4);
                    if chain | 1 == h | 1 && sym_name(i) == Some(name) {
                        result = Some(i);
                        break;
                    }
                    if chain & 1 != 0 {
                        break;
                    }
                    i += 1;
                }
            }
        }
        found.push(result);
    }

    if let Some(sh) = elf
        .section_headers
        .iter()
        .find(|sh| sh.sh_type == goblin::elf::section_header::SHT_HASH)
    {
        let base = sh.sh_offset as usize;
        let nbucket = u32_at(base) as usize;
        let h = sysv_hash(name.as_bytes()) as usize;
        let mut i = u32_at(base + 8 + (h % nbucket) * 4) as usize;
        while i != 0 && sym_name(i) != Some(name) {
            i = u32_at(base + 8 + (nbucket + i) * 4) as usize;
        }
        found.push((i != 0).then_some(i));
    }
    found
}

/// What the dynamic loader sees of a defined dynamic symbol, by name.
fn defined_symbols(elf: &Elf) -> std::collections::BTreeMap<String, (u64, u64, u8, u16)> {
    elf.dynsyms
        .iter()
        .enumerate()
        .filter(|(_, s)| s.st_shndx != 0)
        .map(|(i, s)| {
            let name = elf.dynstrtab.get_at(s.st_name).unwrap().to_string();
            let version = elf
                .versym
                .as_ref()
                .and_then(|v| v.get_at(i))
                .map_or(0, |v| v.vs_val);
            (name, (s.st_value, s.st_size, s.st_info, version))
        })
        .collect()
}

/// Dynamic relocations as (offset, type, addend, symbol name).
fn dynamic_relocations(elf: &Elf) -> Vec<(u64, u32, i64, String)> {
    let mut relocs: Vec<_> = elf
        .dynrelas
        .iter()
        .chain(elf.dynrels.iter())
        .chain(elf.pltrelocs.iter())
        .map(|r| {
            let name = elf
                .dynsyms
                .get(r.r_sym)
                .and_then(|s| elf.dynstrtab.get_at(s.st_name))
                .unwrap_or("")
                .to_string();
            (r.r_offset, r.r_type, r.r_addend.unwrap_or(0), name)
        })
        .collect();
    relocs.sort();
    relocs
}

/// Renaming defined dynamic symbols keeps every symbol reachable through both
/// hash tables under its new name, and keeps symbol values, versions and
/// relocations attached to the right symbols, although the symbols may have to
/// be reordered.
#[rstest]
fn test_rename_dynamic_symbols_keeps_hash_tables_valid(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let before = Elf::parse(&data_bytes).unwrap();
    let mut symbols = defined_symbols(&before);
    let mut candidates: Vec<String> = symbols
        .keys()
        .filter(|name| !name.is_empty())
        .cloned()
        .collect();
    if candidates.is_empty() {
        // Nothing is defined: rename the imports instead.
        candidates = before
            .dynsyms
            .iter()
            .filter_map(|s| before.dynstrtab.get_at(s.st_name))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }
    let renames: std::collections::HashMap<String, String> = candidates
        .iter()
        .step_by(3)
        .map(|name| (name.clone(), format!("{name}_renamed")))
        .collect();
    assert!(!renames.is_empty());
    let mut relocations = dynamic_relocations(&before);

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container.rename_dynamic_symbols(&renames).unwrap();
    let mut out = Vec::new();
    elf_container.write(&mut out).unwrap();
    let after = Elf::parse(&out).unwrap();

    for (old, new) in &renames {
        if let Some(value) = symbols.remove(old) {
            symbols.insert(new.clone(), value);
        }
    }
    for r in &mut relocations {
        if let Some(new) = renames.get(&r.3) {
            r.3 = new.clone();
        }
    }
    relocations.sort();
    assert_eq!(defined_symbols(&after), symbols);
    assert_eq!(dynamic_relocations(&after), relocations);

    for name in symbols.keys().filter(|name| !name.is_empty()) {
        let found = hash_lookups(&after, &out, name);
        assert!(!found.is_empty());
        for index in found {
            let index = index.unwrap_or_else(|| panic!("{name} is not found"));
            assert_eq!(
                after
                    .dynstrtab
                    .get_at(after.dynsyms.get(index).unwrap().st_name),
                Some(name.as_str())
            );
        }
    }
    for old in renames.keys() {
        assert!(
            hash_lookups(&after, &out, old).iter().all(Option::is_none),
            "{old} is still found"
        );
    }
}

/// Mark the section that follows the program headers writable, which arwen
/// does not move, so that the program header table cannot grow in place.
fn pin_section_after_program_headers(data: &mut [u8]) {
    let elf = Elf::parse(data).unwrap();
    let end =
        elf.header.e_phoff + u64::from(elf.header.e_phentsize) * u64::from(elf.header.e_phnum);
    let (index, _) = elf
        .section_headers
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, sh)| sh.sh_offset >= end)
        .min_by_key(|(_, sh)| sh.sh_offset)
        .unwrap();
    let flags = elf.header.e_shoff as usize + index * usize::from(elf.header.e_shentsize) + 8;
    let low_byte = if elf.little_endian {
        flags
    } else {
        flags + if elf.is_64 { 7 } else { 3 }
    };
    data[low_byte] |= 1; // SHF_WRITE
}

/// When the program header table needs another entry but cannot grow where it
/// is, it moves into the new segment and stays reachable the ways the kernel
/// and the dynamic loader find it.
#[rstest]
fn test_program_headers_move_when_blocked(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let mut data_bytes = std::fs::read(&bin_path).unwrap();
    pin_section_after_program_headers(&mut data_bytes);
    let before = Elf::parse(&data_bytes).unwrap();
    // With room for another entry before the next section, the table grows in
    // place instead (e.g. lld leaves a gap before the first section).
    let entry = u64::from(before.header.e_phentsize);
    let table_end = before.header.e_phoff + entry * u64::from(before.header.e_phnum);
    let next_section = before
        .section_headers
        .iter()
        .skip(1)
        .map(|sh| sh.sh_offset)
        .filter(|&offset| offset >= table_end)
        .min()
        .unwrap();
    if next_section - table_end >= entry {
        return;
    }
    let interp = before.interpreter.map(str::to_owned);

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    let runpath = format!("/{}", "p".repeat(5000));
    elf_container.set_runpath(&runpath).unwrap();
    let mut out = Vec::new();
    elf_container.write(&mut out).unwrap();

    let elf = Elf::parse(&out).unwrap();
    assert_ne!(
        elf.header.e_phoff, before.header.e_phoff,
        "the table did not move"
    );
    assert_eq!(elf.program_headers.len(), before.program_headers.len() + 1);
    assert_eq!(
        elf.runpaths
            .iter()
            .chain(&elf.rpaths)
            .copied()
            .collect::<Vec<_>>(),
        [runpath.as_str()]
    );
    assert_eq!(elf.interpreter.map(str::to_owned), interp);

    use goblin::elf::program_header::{PT_INTERP, PT_LOAD, PT_PHDR};
    let table_size = u64::from(elf.header.e_phentsize) * u64::from(elf.header.e_phnum);
    let loads: Vec<_> = elf
        .program_headers
        .iter()
        .filter(|p| p.p_type == PT_LOAD)
        .collect();
    // The table is loaded, at a consistent offset and address.
    let load = loads
        .iter()
        .find(|p| {
            p.p_offset <= elf.header.e_phoff
                && elf.header.e_phoff + table_size <= p.p_offset + p.p_filesz
        })
        .expect("the program headers are not in a PT_LOAD");
    let table_addr = load.p_vaddr + (elf.header.e_phoff - load.p_offset);
    if let Some(phdr) = elf.program_headers.iter().find(|p| p.p_type == PT_PHDR) {
        assert_eq!(phdr.p_offset, elf.header.e_phoff);
        assert_eq!(phdr.p_vaddr, table_addr);
        assert_eq!(phdr.p_filesz, table_size);
    }
    // Kernels before 5.18 compute AT_PHDR from the first PT_LOAD as
    // `p_vaddr - p_offset + e_phoff`.
    if elf.program_headers.iter().any(|p| p.p_type == PT_INTERP) {
        let first = loads.iter().min_by_key(|p| p.p_vaddr).unwrap();
        assert_eq!(
            first
                .p_vaddr
                .wrapping_sub(first.p_offset)
                .wrapping_add(elf.header.e_phoff),
            table_addr
        );
    }
    // Every segment maps its file range at an address congruent modulo its alignment.
    for p in &loads {
        assert_eq!(p.p_vaddr % p.p_align, p.p_offset % p.p_align, "{p:?}");
    }
}

/// Apply a few modifications that grow, move and rewrite different parts of
/// the file. `defined` is a symbol the file defines, if any.
fn modify_a_lot(elf: &mut ElfContainer, round: usize, defined: Option<&str>) {
    let renamed = |name: &str, round: usize| {
        if round == 0 {
            name.to_string()
        } else {
            format!("{name}_r{}", round - 1)
        }
    };
    let mut renames = std::collections::HashMap::new();
    for name in ["setlocale"].into_iter().chain(defined) {
        renames.insert(renamed(name, round), renamed(name, round + 1));
    }
    elf.rename_dynamic_symbols(&renames).unwrap();
    // liblld.so has no symbol versions to clear.
    let cleared = elf.clear_version_symbol(&renamed("setlocale", round + 1));
    assert!(
        matches!(
            cleared,
            Ok(()) | Err(arwen_elf::ElfError::MissingSection(".gnu.version"))
        ),
        "{cleared:?}"
    );
    elf.replace_needed(&std::collections::HashMap::from([(
        renamed("libc.so.6", round),
        renamed("libc.so.6", round + 1),
    )]))
    .unwrap();
    elf.set_runpath(format!("/round/{round}/{}", "q".repeat(300)))
        .unwrap();
    elf.add_needed(vec![format!("libround{round}.so")]).unwrap();
    let _ = elf.set_interpreter(&format!("/lib/{}-{round}.so", "i".repeat(150)));
    elf.set_exec_stack().unwrap();
    elf.add_debug_tag().unwrap();
    // Short enough to overwrite the old soname in place. Only shared objects
    // have one.
    let _ = elf.set_soname(format!("s{round}"));
}

/// Writing only the changed bytes to the original file gives exactly the file
/// a full write gives, also over several rounds of changes.
#[rstest]
fn test_write_changes_matches_full_write(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
    #[values(false, true)] pin: bool,
) {
    let mut data_bytes = std::fs::read(&bin_path).unwrap();
    if pin {
        pin_section_after_program_headers(&mut data_bytes);
    }
    let defined = defined_symbols(&Elf::parse(&data_bytes).unwrap())
        .into_keys()
        .find(|name| !name.is_empty());
    let defined = defined.as_deref();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("patched");
    std::fs::write(&path, &data_bytes).unwrap();

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut in_place = ElfContainer::parse(data_bytes.clone()).unwrap();
    let mut full = ElfContainer::parse(data_bytes).unwrap();
    for round in 0..3 {
        modify_a_lot(&mut in_place, round, defined);
        in_place.write_changes(&mut file).unwrap();
        modify_a_lot(&mut full, round, defined);
        let expected = full.inner.to_bytes().unwrap();
        assert!(
            std::fs::read(&path).unwrap() == expected,
            "round {round}: the file differs from a full write"
        );
    }
}

/// `write_changes` refuses a file whose length changed since it was read.
#[test]
fn test_write_changes_detects_a_changed_file() {
    let data_bytes = std::fs::read("../../tests/data/elf/x64/lib/libfoo.so").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("patched");
    std::fs::write(&path, &data_bytes[..data_bytes.len() - 1]).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut elf = ElfContainer::parse(&data_bytes).unwrap();
    elf.add_debug_tag().unwrap();
    assert!(elf.write_changes(&mut file).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        data_bytes[..data_bytes.len() - 1]
    );
}

/// Symbols defined in a section that moves follow it, in `.symtab` too.
#[test]
fn test_symbols_follow_moved_sections() {
    let data_bytes = std::fs::read("../../tests/data/elf/x64/lib/libfoo-symtab.so").unwrap();
    let before = Elf::parse(&data_bytes).unwrap();
    let dynamic_addr = |elf: &Elf| {
        elf.section_headers
            .iter()
            .find(|sh| sh.sh_type == goblin::elf::section_header::SHT_DYNAMIC)
            .unwrap()
            .sh_addr
    };
    let symbol = |elf: &Elf, name: &str| {
        elf.syms
            .iter()
            .find(|s| elf.strtab.get_at(s.st_name) == Some(name))
            .unwrap()
            .st_value
    };
    assert_eq!(symbol(&before, "_DYNAMIC"), dynamic_addr(&before));

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    // Far more entries than the dynamic table has room for.
    elf_container
        .add_needed((0..64).map(|i| format!("libmany{i}.so")).collect())
        .unwrap();
    let mut out = Vec::new();
    elf_container.write(&mut out).unwrap();
    let after = Elf::parse(&out).unwrap();

    assert_ne!(
        dynamic_addr(&after),
        dynamic_addr(&before),
        ".dynamic did not move"
    );
    assert_eq!(symbol(&after, "_DYNAMIC"), dynamic_addr(&after));
    // Symbols elsewhere stay where they are.
    for name in ["foo_locale", "foo_open", "foo_table"] {
        assert_eq!(symbol(&after, name), symbol(&before, name), "{name}");
    }
}

/// Only shared objects get a soname: setting one on an executable (`ET_EXEC`)
/// fails and leaves the file as it was.
#[rstest]
fn test_set_soname_requires_a_shared_object(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let is_shared = Elf::parse(&data_bytes).unwrap().header.e_type == goblin::elf::header::ET_DYN;

    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    let result = elf_container.set_soname("libnew.so.1");
    let mut out = Vec::new();
    elf_container.write(&mut out).unwrap();
    if is_shared {
        result.unwrap();
        assert_eq!(Elf::parse(&out).unwrap().soname, Some("libnew.so.1"));
    } else {
        assert!(result.is_err());
        assert_eq!(out, data_bytes);
    }
}

/// `replace-needed` must also rename the library in the version requirements
/// (`.gnu.version_r`): the dynamic loader looks the versions up in the loaded
/// object of that name.
#[rstest]
fn test_replace_needed_renames_version_requirements(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    if Elf::parse(&data_bytes).unwrap().verneed.is_none() {
        // No version requirements to rename.
        return;
    }
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container
        .replace_needed(&std::collections::HashMap::from([(
            "libc.so.6",
            "libnotc.so.6",
        )]))
        .unwrap();
    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let version_files = |data: &[u8]| -> Vec<String> {
        let elf = Elf::parse(data).unwrap();
        let verneed = elf
            .verneed
            .as_ref()
            .expect("fixture has version requirements");
        verneed
            .iter()
            .map(|need| elf.dynstrtab.get_at(need.vn_file).unwrap().to_string())
            .collect()
    };
    let expected: Vec<String> = version_files(&data_bytes)
        .into_iter()
        .map(|file| {
            if file == "libc.so.6" {
                "libnotc.so.6".to_string()
            } else {
                file
            }
        })
        .collect();
    assert!(expected.contains(&"libnotc.so.6".to_string()));
    assert_eq!(version_files(&changed_elf_data), expected);
}

/// Changing the search path of a file with only a `DT_RPATH` turns it into a
/// `DT_RUNPATH`.
#[rstest]
fn test_lone_rpath_becomes_runpath(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
    #[values("set", "add", "shrink")] operation: &str,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    // Start from a lone DT_RPATH.
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container.set_runpath("/opt/old/lib:$ORIGIN").unwrap();
    elf_container.force_rpath().unwrap();
    let mut rpath_only = Vec::new();
    elf_container.write(&mut rpath_only).unwrap();
    let before = Elf::parse(&rpath_only).unwrap();
    assert!(before.runpaths.is_empty());
    assert_eq!(before.rpaths, vec!["/opt/old/lib:$ORIGIN"]);

    let mut elf_container = ElfContainer::parse(&rpath_only).unwrap();
    let expected = match operation {
        "set" => {
            elf_container.set_runpath("/new/lib").unwrap();
            "/new/lib".to_string()
        }
        "add" => {
            elf_container.add_runpath("/new/lib").unwrap();
            "/opt/old/lib:$ORIGIN:/new/lib".to_string()
        }
        "shrink" => {
            // /opt/old/lib does not exist; relative entries are kept.
            elf_container.shrink_rpath(Vec::new()).unwrap();
            "$ORIGIN".to_string()
        }
        _ => unreachable!(),
    };
    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed = Elf::parse(&changed_elf_data).unwrap();
    assert!(changed.rpaths.is_empty(), "{:?}", changed.rpaths);
    assert_eq!(changed.runpaths, vec![expected.as_str()]);
}

/// Linked with lld: no `DT_NEEDED`, no search path and no symbol versions.
const LLD_LIB: &str = "../../tests/data/elf/x64/lib/liblld.so";

/// `clear-symbol-version` fails on a file without symbol versions.
#[test]
fn test_clear_symbol_version_requires_symbol_versions() {
    let data_bytes = std::fs::read(LLD_LIB).unwrap();
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    assert!(matches!(
        elf_container.clear_version_symbol("bar_next"),
        Err(arwen_elf::ElfError::MissingSection(".gnu.version"))
    ));
}

/// `shrink-rpath` leaves a file without a search path alone.
#[test]
fn test_shrink_rpath_without_search_path() {
    let data_bytes = std::fs::read(LLD_LIB).unwrap();
    let mut elf_container = ElfContainer::parse(&data_bytes).unwrap();
    elf_container.shrink_rpath(Vec::new()).unwrap();
    let mut changed_elf_data = Vec::new();
    elf_container.write(&mut changed_elf_data).unwrap();

    let changed = Elf::parse(&changed_elf_data).unwrap();
    assert!(changed.rpaths.is_empty());
    assert!(changed.runpaths.is_empty());
}

/// Extend `DT_RELASZ` (or `DT_RELSZ`) over the PLT relocations that directly
/// follow, so that both dynamic tags cover them, as some linkers emit. Returns
/// whether the file has such adjacent tables.
fn overlap_relocations_with_plt(data: &mut [u8]) -> bool {
    use goblin::elf::dynamic::{DT_JMPREL, DT_PLTRELSZ, DT_REL, DT_RELA, DT_RELASZ, DT_RELSZ};
    let elf = Elf::parse(data).unwrap();
    let Some(dynamic) = &elf.dynamic else {
        return false;
    };
    let value = |tag| {
        dynamic
            .dyns
            .iter()
            .find(|d| d.d_tag == tag)
            .map(|d| d.d_val)
    };
    let (Some(jmprel), Some(pltrelsz)) = (value(DT_JMPREL), value(DT_PLTRELSZ)) else {
        return false;
    };
    let Some((size_tag, size)) = [(DT_RELA, DT_RELASZ), (DT_REL, DT_RELSZ)]
        .into_iter()
        .find_map(|(addr, size)| match (value(addr), value(size)) {
            (Some(a), Some(s)) if a + s == jmprel && s > 0 => Some((size, s)),
            _ => None,
        })
    else {
        return false;
    };
    let index = dynamic
        .dyns
        .iter()
        .position(|d| d.d_tag == size_tag)
        .unwrap();
    let phdr = elf
        .program_headers
        .iter()
        .find(|p| p.p_type == goblin::elf::program_header::PT_DYNAMIC)
        .unwrap();
    let word = if elf.is_64 { 8 } else { 4 };
    let at = phdr.p_offset as usize + index * 2 * word + word;
    let new = size + pltrelsz;
    let bytes = match (elf.is_64, elf.little_endian) {
        (true, true) => new.to_le_bytes().to_vec(),
        (true, false) => new.to_be_bytes().to_vec(),
        (false, true) => (new as u32).to_le_bytes().to_vec(),
        (false, false) => (new as u32).to_be_bytes().to_vec(),
    };
    data[at..at + word].copy_from_slice(&bytes);
    true
}

/// Reordering the dynamic symbols rewrites each relocation once, also when
/// the dynamic tags describe overlapping relocation tables.
#[rstest]
fn test_rename_with_overlapping_relocation_tables(
    #[files("../../tests/data/elf/*/*/*")]
    #[files("../../tests/data/elf-big-endian/*/*/*")]
    bin_path: PathBuf,
) {
    let mut data_bytes = std::fs::read(&bin_path).unwrap();
    if !overlap_relocations_with_plt(&mut data_bytes) {
        return;
    }
    let before = Elf::parse(&data_bytes).unwrap();
    let renames: std::collections::HashMap<String, String> = before
        .dynsyms
        .iter()
        .filter_map(|s| before.dynstrtab.get_at(s.st_name))
        .filter(|name| !name.is_empty())
        .step_by(2)
        .map(|name| (name.to_string(), format!("{name}_renamed")))
        .collect();
    let mut relocations = dynamic_relocations(&before);
    assert!(relocations.iter().any(|r| renames.contains_key(&r.3)));

    let mut elf = ElfContainer::parse(&data_bytes).unwrap();
    elf.rename_dynamic_symbols(&renames).unwrap();
    let mut out = Vec::new();
    elf.write(&mut out).unwrap();
    let after = Elf::parse(&out).unwrap();

    for r in &mut relocations {
        if let Some(new) = renames.get(&r.3) {
            r.3 = new.clone();
        }
    }
    relocations.sort();
    assert_eq!(dynamic_relocations(&after), relocations);
}
