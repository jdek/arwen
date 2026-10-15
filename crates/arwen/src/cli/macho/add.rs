use std::path::PathBuf;

use clap::Parser;

use arwen_macho::MachoError;

/// Add a run path
#[derive(Parser, Debug)]
pub struct Args {
    /// New rpath to add
    pub new_rpath: String,

    /// Path to the file to change
    pub path_to_binary: PathBuf,
}

pub fn execute(args: Args) -> Result<(), MachoError> {
    super::patch(&args.path_to_binary, |macho| {
        macho.add_rpath(&args.new_rpath)?;
        Ok(())
    })
}
