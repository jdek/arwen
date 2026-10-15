use std::path::PathBuf;

use clap::Parser;

use arwen_macho::MachoError;

/// Remove a run path
#[derive(Parser, Debug)]
pub struct Args {
    /// Rpath to remove
    pub rpath_to_remove: String,

    /// Path to the file to change
    pub path_to_binary: PathBuf,
}

pub fn execute(args: Args) -> Result<(), MachoError> {
    super::patch(&args.path_to_binary, |macho| {
        macho.remove_rpath(&args.rpath_to_remove)?;
        Ok(())
    })
}
