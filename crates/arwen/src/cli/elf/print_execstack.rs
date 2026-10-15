use std::path::PathBuf;

use clap::Parser;

/// Print the exec stack of the ELF file: `execstack: X`, `execstack: -`, or `execstack: ?` without PT_GNU_STACK
#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the file to change
    pub path_to_binary: PathBuf,
}

pub fn execute(args: Args) -> Result<(), arwen_elf::ElfError> {
    let mut elf = super::read(&args.path_to_binary)?;

    elf.print_exec_stack();

    Ok(())
}
