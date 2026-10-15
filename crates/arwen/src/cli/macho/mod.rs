pub mod add;
pub mod change;
pub mod codesign;
pub mod delete;
pub mod install_id;
pub mod install_name;

use std::{fs::OpenOptions, path::Path};

use super::MachoCommand;
use arwen_macho::{MachoError, MachoFile};

pub fn execute(macho: MachoCommand) -> Result<(), MachoError> {
    match macho {
        MachoCommand::DeleteRpath(args) => delete::execute(args),
        MachoCommand::ChangeRpath(args) => change::execute(args),
        MachoCommand::AddRpath(args) => add::execute(args),
        MachoCommand::ChangeInstallName(args) => install_name::execute(args),
        MachoCommand::ChangeInstallId(args) => install_id::execute(args),
        MachoCommand::AdhocSign(args) => codesign::execute(args),
    }
}

/// Apply `edit` to the Mach-O file at `path`, writing back only the bytes
/// that change.
fn patch(
    path: &Path,
    edit: impl FnOnce(&mut MachoFile) -> Result<(), MachoError>,
) -> Result<(), MachoError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut macho = MachoFile::read(&mut file)?;
    edit(&mut macho)?;
    macho.write_changes(&mut file)
}
