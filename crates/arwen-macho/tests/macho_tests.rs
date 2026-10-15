use arwen_macho::MachoContainer;
use goblin::mach::MachO;
use rstest::rstest;
use std::path::PathBuf;

macro_rules! set_snapshot_suffix {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(format!($($expr,)*));
        let _guard = settings.bind_to_scope();
    }
}

/// This test checks if the rpath of a Mach-O file can be changed.
#[rstest]
fn test_rpath_change(#[files("../../tests/data/macho/x64/exec/*")] bin_path: PathBuf) {
    set_snapshot_suffix!("{}", bin_path.file_name().unwrap().to_str().unwrap());

    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    macho_container
        .change_rpath("path_graf", "path_graf_path_graf_path_graf_path_graf")
        .unwrap();

    let changed_macho = MachO::parse(&macho_container.data, 0).unwrap();

    insta::assert_debug_snapshot!(changed_macho);
    insta::assert_debug_snapshot!(changed_macho.rpaths);
    insta::assert_snapshot!(macho_container.data.len());
}

/// This test checks if the rpath of a Mach-O file can be removed.
#[rstest]
fn test_rpath_remove(#[files("../../tests/data/macho/x64/exec/*")] bin_path: PathBuf) {
    set_snapshot_suffix!("{}", bin_path.file_name().unwrap().to_str().unwrap());
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    macho_container.remove_rpath("path_graf").unwrap();

    let changed_macho = MachO::parse(&macho_container.data, 0).unwrap();

    insta::assert_debug_snapshot!(changed_macho);
    insta::assert_debug_snapshot!(changed_macho.rpaths);
    insta::assert_snapshot!(macho_container.data.len());
}

/// This test verify that a rpath can be added to a Mach-O file.
#[rstest]
fn test_rpath_add(#[files("../../tests/data/macho/x64/exec/*")] bin_path: PathBuf) {
    set_snapshot_suffix!("{}", bin_path.file_name().unwrap().to_str().unwrap());
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    macho_container
        .add_rpath("abababababababababababaabbababababababababab")
        .unwrap();

    let changed_macho = MachO::parse(&macho_container.data, 0).unwrap();

    insta::assert_debug_snapshot!(changed_macho);
    insta::assert_debug_snapshot!(changed_macho.rpaths);
    insta::assert_snapshot!(macho_container.data.len());
}

/// This test verify that an dylib id can be changed in a Mach-O file.
#[rstest]
fn test_change_dylib_id(#[files("../../tests/data/macho/x64/libs/*")] lib_path: PathBuf) {
    let data_bytes = std::fs::read(&lib_path).unwrap();

    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    macho_container
        .change_install_id("very_very_very_very_very_very_very_longid.dylib")
        .unwrap();

    let changed_macho = MachO::parse(&macho_container.data, 0).unwrap();

    insta::assert_debug_snapshot!(changed_macho);
    insta::assert_debug_snapshot!(changed_macho.name);
    insta::assert_snapshot!(macho_container.data.len());
}

/// This test verify that a name of dynamic library can be changed
#[rstest]
fn test_change_dylib_name(#[files("../../tests/data/macho/x64/exec/*")] bin_path: PathBuf) {
    set_snapshot_suffix!("{}", bin_path.file_name().unwrap().to_str().unwrap());
    let data_bytes = std::fs::read(&bin_path).unwrap();

    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    macho_container
        .change_install_name(
            "/usr/lib/libSystem.B.dylib",
            "very_very_very_very_very_very_very_longid",
        )
        .unwrap();

    let changed_macho = MachO::parse(&macho_container.data, 0).unwrap();

    insta::assert_debug_snapshot!(changed_macho);
    insta::assert_debug_snapshot!(changed_macho.libs);
    insta::assert_snapshot!(macho_container.data.len());
}

/// A load command that does not fit before the first section's data is
/// refused, and the file is left as it was.
#[rstest]
fn test_no_space_for_load_commands(#[files("../../tests/data/macho/*/exec/*")] bin_path: PathBuf) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let mut macho_container = MachoContainer::parse(&data_bytes).unwrap();

    let rpath = format!("/{}", "x".repeat(64 * 1024));
    let result = macho_container.add_rpath(&rpath);

    assert!(
        matches!(result, Err(arwen_macho::MachoError::NoSpace { .. })),
        "{result:?}"
    );
    assert_eq!(&*macho_container.data, &data_bytes[..]);
}

fn first_rpath(data: &[u8]) -> Option<String> {
    MachO::parse(data, 0)
        .ok()
        .and_then(|m| m.rpaths.first().map(|r| r.to_string()))
}

/// Every kind of edit after an added rpath. Install id and name edits fail on
/// files without that command, the same way for both container types.
fn edit_rest(macho: &mut impl Edit, rpath: Option<&str>) {
    if let Some(rpath) = rpath {
        macho.change_rpath(rpath, "/opt/changed/lib").unwrap();
        macho.remove_rpath("/opt/changed/lib").unwrap();
    }
    let _ = macho.change_install_id("@rpath/libchanged.dylib");
    let _ = macho.change_install_name("/usr/lib/libSystem.B.dylib", "/usr/lib/libOther.dylib");
}

/// The edit operations of both container types.
trait Edit {
    fn change_rpath(&mut self, old: &str, new: &str) -> Result<(), arwen_macho::MachoError>;
    fn remove_rpath(&mut self, rpath: &str) -> Result<(), arwen_macho::MachoError>;
    fn change_install_id(&mut self, id: &str) -> Result<(), arwen_macho::MachoError>;
    fn change_install_name(&mut self, old: &str, new: &str) -> Result<(), arwen_macho::MachoError>;
}

macro_rules! impl_edit {
    ($t:ty) => {
        impl Edit for $t {
            fn change_rpath(
                &mut self,
                old: &str,
                new: &str,
            ) -> Result<(), arwen_macho::MachoError> {
                <$t>::change_rpath(self, old, new)
            }
            fn remove_rpath(&mut self, rpath: &str) -> Result<(), arwen_macho::MachoError> {
                <$t>::remove_rpath(self, rpath)
            }
            fn change_install_id(&mut self, id: &str) -> Result<(), arwen_macho::MachoError> {
                <$t>::change_install_id(self, id)
            }
            fn change_install_name(
                &mut self,
                old: &str,
                new: &str,
            ) -> Result<(), arwen_macho::MachoError> {
                <$t>::change_install_name(self, old, new)
            }
        }
    };
}
impl_edit!(MachoContainer<'_>);
impl_edit!(arwen_macho::MachoFile);

/// Editing a file in place with `MachoFile` gives exactly the bytes that
/// `MachoContainer` gives in memory, also when writing more than once.
#[rstest]
fn test_write_changes_matches_container(
    #[files("../../tests/data/macho/*/*/*")] bin_path: PathBuf,
) {
    let data_bytes = std::fs::read(&bin_path).unwrap();
    let rpath = first_rpath(&data_bytes);
    let mut in_memory = MachoContainer::parse(&data_bytes).unwrap();
    in_memory.add_rpath("/opt/added/lib").unwrap();
    edit_rest(&mut in_memory, rpath.as_deref());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, &data_bytes).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut in_place = arwen_macho::MachoFile::read(&mut file).unwrap();
    in_place.add_rpath("/opt/added/lib").unwrap();
    in_place.write_changes(&mut file).unwrap();
    // The rest of the edits on top, written separately.
    let mut rest = arwen_macho::MachoFile::read(&mut file).unwrap();
    edit_rest(&mut rest, rpath.as_deref());
    rest.write_changes(&mut file).unwrap();
    drop(file);

    assert_eq!(std::fs::read(&path).unwrap(), &*in_memory.data);
}

/// `write_changes` refuses a file whose length changed since it was read.
#[test]
fn test_write_changes_detects_a_changed_file() {
    let data_bytes = std::fs::read("../../tests/data/macho/x64/exec/hello_with_rpath").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, &data_bytes).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut macho = arwen_macho::MachoFile::read(&mut file).unwrap();
    macho.add_rpath("/opt/added/lib").unwrap();
    file.set_len(data_bytes.len() as u64 + 1).unwrap();

    assert!(matches!(
        macho.write_changes(&mut file),
        Err(arwen_macho::MachoError::FileChanged { .. })
    ));
}
