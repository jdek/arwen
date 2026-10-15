//! Writing the modified file back.

use std::fs;
use std::path::Path;
use std::process::Command;

use goblin::elf::{program_header::PT_LOAD, Elf};
use rstest::rstest;
use tempfile::tempdir;

use super::{FILE, LONG_PATH};
use crate::common::ARWEN;

fn unchanged(_: &mut [u8]) {}

/// Move the last `PT_LOAD` of a 32-bit little-endian file to the top of the
/// address space, so that there is no room for a new segment after it.
fn fill_address_space(data: &mut [u8]) {
    let elf = Elf::parse(data).unwrap();
    assert!(!elf.is_64 && elf.little_endian);
    let (index, last) = elf
        .program_headers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.p_type == PT_LOAD)
        .max_by_key(|(_, p)| p.p_vaddr)
        .unwrap();
    let align = last.p_align.max(1);
    let vaddr = ((u64::from(u32::MAX) - last.p_memsz) / align - 1) * align + last.p_offset % align;
    let at = elf.header.e_phoff as usize + index * usize::from(elf.header.e_phentsize);
    // p_vaddr and p_paddr of an Elf32_Phdr.
    data[at + 8..at + 12].copy_from_slice(&(vaddr as u32).to_le_bytes());
    data[at + 12..at + 16].copy_from_slice(&(vaddr as u32).to_le_bytes());
}

/// A command that fails leaves the file as it was, whether it fails while
/// editing or while laying out the new file.
#[rstest]
#[case::not_a_shared_object(
    "../../tests/data/elf/x64/exec/linux-x64-bash",
    unchanged,
    &["set-soname", "libnew.so", FILE],
    "not a shared object",
)]
#[case::no_room_for_a_new_segment(
    "../../tests/data/elf/x32/exec/linux-x32-bash",
    fill_address_space,
    &["set-rpath", LONG_PATH, FILE],
    "does not fit in the 32-bit address space",
)]
fn failed_modification_leaves_the_file_untouched(
    #[case] bin: &str,
    #[case] prepare: fn(&mut [u8]),
    #[case] args: &[&str],
    #[case] error: &str,
) {
    let dir = tempdir().unwrap();
    let file = dir.path().join(Path::new(bin).file_name().unwrap());
    let mut original = fs::read(bin).unwrap();
    prepare(&mut original);
    fs::write(&file, &original).unwrap();

    let output = Command::new(ARWEN)
        .arg("elf")
        .args(args.iter().map(|arg| {
            if *arg == FILE {
                file.as_os_str().to_owned()
            } else {
                arg.into()
            }
        }))
        .output()
        .unwrap();

    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(error), "unexpected error: {stderr}");
    assert_eq!(fs::read(&file).unwrap(), original);
}
