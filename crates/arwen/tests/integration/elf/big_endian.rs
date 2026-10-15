//! Big-endian fixtures (`tests/data/elf-big-endian`, built by
//! `tools/elf/big-endian/build.sh`).
//!
//! patchelf 0.19 is not a reliable oracle for these: when it has to grow the
//! program header table of a big-endian non-PIE executable it writes a corrupt
//! table, and `--no-default-lib` stores `DF_1_NODEFLIB` byte-swapped. So arwen's
//! output is checked against the input with the expected change applied, and in
//! addition against patchelf's output whenever that is structurally valid.

use std::fs;
use std::path::{Path, PathBuf};

use goblin::elf::{program_header, section_header, Elf};
use rstest::rstest;
use tempfile::tempdir;

use super::{
    patchelf, prepare, try_run, ElfSummary, SymbolVersion, FILE, LONG_PATH, ORIG_RPATH, RENAME_MAP,
};
use crate::common::ARWEN;

const DF_1_NODEFLIB: u64 = 0x800;
const PF_X: u32 = 1;

/// What would make the file unloadable: every allocated section, the program
/// header table and the interpreter and dynamic segments must be mapped by a
/// `PT_LOAD`, at the address their headers say, and the loads must be sorted.
fn structural_problems(path: &Path) -> Vec<String> {
    let data = fs::read(path).unwrap();
    let elf = match Elf::parse(&data) {
        Ok(elf) => elf,
        Err(e) => return vec![format!("cannot parse: {e}")],
    };
    let loads: Vec<_> = elf
        .program_headers
        .iter()
        .filter(|p| p.p_type == program_header::PT_LOAD)
        .collect();
    let mut problems = Vec::new();
    if loads.windows(2).any(|w| w[0].p_vaddr > w[1].p_vaddr) {
        problems.push("PT_LOAD entries are not sorted by address".to_string());
    }
    // Is the file range [offset, offset + size) mapped at `addr`?
    let mapped = |offset: u64, addr: u64, size: u64| {
        loads.iter().any(|p| {
            p.p_offset <= offset
                && offset + size <= p.p_offset + p.p_filesz
                && addr.wrapping_sub(p.p_vaddr) == offset - p.p_offset
        })
    };
    for p in &loads {
        if p.p_align > 1 && p.p_offset % p.p_align != p.p_vaddr % p.p_align {
            problems.push(format!("misaligned PT_LOAD {p:?}"));
        }
        if p.p_offset + p.p_filesz > data.len() as u64 {
            problems.push(format!("PT_LOAD past the end of the file {p:?}"));
        }
    }
    for p in &elf.program_headers {
        let checked = matches!(
            p.p_type,
            program_header::PT_PHDR | program_header::PT_INTERP | program_header::PT_DYNAMIC
        );
        if checked && !mapped(p.p_offset, p.p_vaddr, p.p_filesz) {
            problems.push(format!("segment not mapped: {p:?}"));
        }
    }
    if let Some(interp) = elf
        .program_headers
        .iter()
        .find(|p| p.p_type == program_header::PT_INTERP)
    {
        let end = (interp.p_offset + interp.p_filesz) as usize;
        if data.get(end - 1) != Some(&0) {
            problems.push("PT_INTERP does not end with a NUL".to_string());
        }
    }
    for (i, sh) in elf.section_headers.iter().enumerate() {
        let alloc = sh.sh_flags & u64::from(section_header::SHF_ALLOC) != 0;
        if alloc
            && sh.sh_type != section_header::SHT_NOBITS
            && sh.sh_size > 0
            && !mapped(sh.sh_offset, sh.sh_addr, sh.sh_size)
        {
            problems.push(format!("section {i} not mapped"));
        }
    }
    problems
}

/// Expected changes, applied to the input's summary. Returns `false` if the
/// operation must fail on this input.
type Expect = fn(&mut ElfSummary) -> bool;

fn set_search_path(s: &mut ElfSummary, value: String) {
    // A lone DT_RPATH becomes a DT_RUNPATH.
    if s.runpath.is_empty() {
        s.rpath.clear();
        s.runpath = vec![value];
    } else {
        if !s.rpath.is_empty() {
            s.rpath = vec![value.clone()];
        }
        s.runpath = vec![value];
    }
}

fn rename_symbol(s: &mut ElfSummary, old: &str, new: &str) {
    for (name, _) in &mut s.dynamic_symbols {
        if name == old {
            *name = new.to_string();
        }
    }
    s.dynamic_symbols.sort();
}

#[rstest]
#[case::set_interpreter_long(
    &["set-interpreter", LONG_PATH, FILE],
    &["--set-interpreter", LONG_PATH, FILE],
    |s: &mut ElfSummary| s.interpreter.replace(LONG_PATH.into()).is_some(),
)]
#[case::set_interpreter_short(
    &["set-interpreter", "/ld.so", FILE],
    &["--set-interpreter", "/ld.so", FILE],
    |s: &mut ElfSummary| s.interpreter.replace("/ld.so".into()).is_some(),
)]
#[case::set_rpath_long(
    &["set-rpath", LONG_PATH, FILE],
    &["--set-rpath", LONG_PATH, FILE],
    |s: &mut ElfSummary| { set_search_path(s, LONG_PATH.into()); true },
)]
#[case::add_rpath(
    &["add-rpath", "/opt/added/lib", FILE],
    &["--add-rpath", "/opt/added/lib", FILE],
    |s: &mut ElfSummary| {
        let old = s.runpath.first().or(s.rpath.first()).cloned().unwrap_or_default();
        set_search_path(s, format!("{old}:/opt/added/lib"));
        true
    },
)]
#[case::remove_rpath(&["remove-rpath", FILE], &["--remove-rpath", FILE], |s: &mut ElfSummary| {
    s.rpath.clear();
    s.runpath.clear();
    true
})]
#[case::force_rpath(
    &["force-rpath", FILE],
    &["--force-rpath", "--set-rpath", ORIG_RPATH, FILE],
    |s: &mut ElfSummary| {
        s.rpath.append(&mut s.runpath);
        true
    },
)]
#[case::add_needed(
    &["add-needed", FILE, "libbar.so", "libfoo.so"],
    &["--add-needed", "libbar.so", "--add-needed", "libfoo.so", FILE],
    |s: &mut ElfSummary| { s.needed.splice(0..0, ["libbar.so".into(), "libfoo.so".into()]); true },
)]
#[case::remove_needed(
    &["remove-needed", FILE, "libdl.so.2"],
    &["--remove-needed", "libdl.so.2", FILE],
    |s: &mut ElfSummary| { s.needed.retain(|n| n != "libdl.so.2"); true },
)]
#[case::replace_needed(
    &["replace-needed", FILE, "libc.so.6=libnotc.so.6"],
    &["--replace-needed", "libc.so.6", "libnotc.so.6", FILE],
    |s: &mut ElfSummary| {
        for n in &mut s.needed {
            if n == "libc.so.6" {
                *n = "libnotc.so.6".into();
            }
        }
        for (_, version) in &mut s.dynamic_symbols {
            if let SymbolVersion::Needed { file, .. } = version {
                if file == "libc.so.6" {
                    *file = "libnotc.so.6".into();
                }
            }
        }
        true
    },
)]
#[case::set_soname(
    &["set-soname", "libnew.so", FILE],
    &["--set-soname", "libnew.so", FILE],
    |s: &mut ElfSummary| {
        // Only shared objects have a soname; arwen refuses the others.
        if s.e_type != goblin::elf::header::ET_DYN {
            return false;
        }
        s.soname = Some("libnew.so".into());
        true
    },
)]
#[case::set_os_abi(&["set-os-abi", "freebsd", FILE], &["--set-os-abi", "freebsd", FILE], |s: &mut ElfSummary| {
    s.os_abi = 9;
    true
})]
#[case::no_default_lib(&["no-default-lib", FILE], &["--no-default-lib", FILE], |s: &mut ElfSummary| {
    s.flags_1 = Some(s.flags_1.unwrap_or(0) | DF_1_NODEFLIB);
    true
})]
#[case::add_debug_tag(&["add-debug-tag", FILE], &["--add-debug-tag", FILE], |s: &mut ElfSummary| {
    s.has_debug = true;
    true
})]
#[case::set_execstack(&["set-exec-stack", FILE], &["--set-execstack", FILE], |s: &mut ElfSummary| {
    s.gnu_stack_flags = Some(s.gnu_stack_flags.unwrap_or(0) | PF_X);
    true
})]
#[case::clear_execstack(&["clear-exec-stack", FILE], &["--clear-execstack", FILE], |s: &mut ElfSummary| {
    s.gnu_stack_flags = Some(s.gnu_stack_flags.unwrap_or(0) & !PF_X);
    true
})]
#[case::clear_symbol_version(
    &["clear-symbol-version", "setlocale", FILE],
    &["--clear-symbol-version", "setlocale", FILE],
    |s: &mut ElfSummary| {
        for (name, version) in &mut s.dynamic_symbols {
            if name == "setlocale" {
                *version = SymbolVersion::Global;
            }
        }
        s.dynamic_symbols.sort();
        true
    },
)]
#[case::rename_dynamic_symbols(
    &["rename-dynamic-symbols", FILE, "setlocale=my_setlocale"],
    &["--rename-dynamic-symbols", RENAME_MAP, FILE],
    |s: &mut ElfSummary| { rename_symbol(s, "setlocale", "my_setlocale"); true },
)]
#[case::rename_defined_symbol(
    &["rename-dynamic-symbols", FILE, "foo_open=foo_open_renamed_to_a_longer_name"],
    &[],
    |s: &mut ElfSummary| { rename_symbol(s, "foo_open", "foo_open_renamed_to_a_longer_name"); true },
)]
fn big_endian_modify(
    #[case] arwen_args: &[&str],
    #[case] patchelf_args: &[&str],
    #[case] expect: Expect,
    #[files("../../tests/data/elf-big-endian/*/*/*")] bin: PathBuf,
) {
    let dir = tempdir().unwrap();
    let (arwen_file, arwen_args) = prepare(&bin, dir.path(), "arwen.bin", arwen_args);
    let arwen_args: Vec<String> = ["elf".to_string()].into_iter().chain(arwen_args).collect();

    let mut expected = ElfSummary::read(&bin);
    let should_succeed = expect(&mut expected);
    let result = try_run(ARWEN, &arwen_args);
    if !should_succeed {
        assert!(result.is_err(), "arwen succeeded but should have failed");
        assert_eq!(fs::read(&arwen_file).unwrap(), fs::read(&bin).unwrap());
        return;
    }
    result.unwrap_or_else(|failure| panic!("{failure}"));

    let problems = structural_problems(&arwen_file);
    assert!(
        problems.is_empty(),
        "arwen wrote a broken file: {problems:?}"
    );
    let actual = ElfSummary::read(&arwen_file);
    let differences = actual.differences(&expected);
    assert!(
        differences.is_empty(),
        "arwen did not make the expected change (\"patchelf\" is the expectation):\n{}",
        differences.join("\n")
    );

    // Compare with patchelf too, where its result can be trusted.
    if patchelf_args.is_empty() || arwen_args[1] == "no-default-lib" {
        return;
    }
    let (patchelf_file, patchelf_args) = prepare(&bin, dir.path(), "patchelf.bin", patchelf_args);
    if try_run(&patchelf(), &patchelf_args).is_err()
        || !structural_problems(&patchelf_file).is_empty()
    {
        return;
    }
    let differences = actual.differences(&ElfSummary::read(&patchelf_file));
    assert!(
        differences.is_empty(),
        "arwen and patchelf disagree:\n{}",
        differences.join("\n")
    );
}

/// The structural check finds the corruption patchelf 0.19 produces when it
/// grows the program header table of a big-endian executable, so that skipping
/// the comparison with patchelf above is justified.
#[test]
fn structural_check_rejects_patchelf_big_endian_output() {
    let dir = tempdir().unwrap();
    let bin = Path::new("../../tests/data/elf-big-endian/ppc64/exec/hello");
    let (file, args) = prepare(
        bin,
        dir.path(),
        "patchelf.bin",
        &["--set-rpath", LONG_PATH, FILE],
    );
    try_run(&patchelf(), &args).unwrap();
    assert!(!structural_problems(&file).is_empty());
    assert!(structural_problems(bin).is_empty());
}
