use std::fs::{self, OpenOptions};
use std::path::Path;

use arwen_elf::{ElfContainer, ElfError};

use super::ElfCommand;

pub mod add_debug_tag;
pub mod add_needed;
pub mod add_rpath;
pub mod clear_execstack;
pub mod clear_version_symbol;
pub mod force_rpath;
pub mod no_default_lib;
pub mod print_execstack;
pub mod print_interpreter;
pub mod print_needed;
pub mod print_os_abi;
pub mod print_rpath;
pub mod print_soname;
pub mod remove_needed;
pub mod remove_rpath;
pub mod rename_dynamic_symbols;
pub mod replace_needed;
pub mod set_execstack;
pub mod set_interpreter;
pub mod set_os_abi;
pub mod set_page_size;
pub mod set_rpath;
pub mod set_soname;
pub mod shrink_rpath;

pub fn execute(elf: ElfCommand) -> Result<(), ElfError> {
    match elf {
        ElfCommand::AddRpath(args) => add_rpath::execute(args),
        ElfCommand::RemoveRpath(args) => remove_rpath::execute(args),
        ElfCommand::SetRpath(args) => set_rpath::execute(args),
        ElfCommand::ForceRpath(args) => force_rpath::execute(args),
        ElfCommand::PrintRpath(args) => print_rpath::execute(args),
        ElfCommand::SetInterpreter(args) => set_interpreter::execute(args),
        ElfCommand::PrintInterpreter(args) => print_interpreter::execute(args),
        ElfCommand::SetOsAbi(args) => set_os_abi::execute(args),
        ElfCommand::PrintOsAbi(args) => print_os_abi::execute(args),
        ElfCommand::SetSoname(args) => set_soname::execute(args),
        ElfCommand::PrintSoname(args) => print_soname::execute(args),
        ElfCommand::ShrinkRpath(args) => shrink_rpath::execute(args),
        ElfCommand::AddNeeded(args) => add_needed::execute(args),
        ElfCommand::RemoveNeeded(args) => remove_needed::execute(args),
        ElfCommand::ReplaceNeeded(args) => replace_needed::execute(args),
        ElfCommand::PrintNeeded(args) => print_needed::execute(args),
        ElfCommand::NoDefaultLib(args) => no_default_lib::execute(args),
        ElfCommand::ClearSymbolVersion(args) => clear_version_symbol::execute(args),
        ElfCommand::RenameDynamicSymbols(args) => rename_dynamic_symbols::execute(args),
        ElfCommand::AddDebugTag(args) => add_debug_tag::execute(args),
        ElfCommand::ClearExecStack(args) => clear_execstack::execute(args),
        ElfCommand::SetExecStack(args) => set_execstack::execute(args),
        ElfCommand::PrintExecStack(args) => print_execstack::execute(args),
        ElfCommand::SetPageSize(args) => set_page_size::execute(args),
    }
}

/// Patch the ELF file at `path` in place: apply `edit`, then write back only
/// the bytes that changed. The file keeps its inode, permissions and links, and
/// is not written to if `edit` or laying out the result fails.
fn patch(
    path: &Path,
    edit: impl FnOnce(&mut ElfContainer) -> Result<(), ElfError>,
) -> Result<(), ElfError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut elf = ElfContainer::read_file(&file)?;
    edit(&mut elf)?;
    elf.write_changes(&mut file)
}

/// Read the ELF file at `path`, only as far as the queries made on it need.
fn read(path: &Path) -> Result<ElfContainer<'static>, ElfError> {
    ElfContainer::read_file(&fs::File::open(path)?)
}
