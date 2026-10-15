use std::path::PathBuf;

use clap::Parser;

use arwen_macho::MachoError;

/// Change existing dylib load name
#[derive(Parser, Debug)]
pub struct Args {
    /// Old rpath to remove
    pub old_install_name: String,

    /// New rpath to add
    pub new_install_name: String,

    /// Path to the file to change
    pub path_to_binary: PathBuf,
}

pub fn execute(args: Args) -> Result<(), MachoError> {
    super::patch(&args.path_to_binary, |macho| {
        macho.change_install_name(&args.old_install_name, &args.new_install_name)?;
        Ok(())
    })
}
