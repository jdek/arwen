//! Editing the load commands of Mach-O files.
//!
//! Every supported edit (rpaths, install names, the dylib id) only changes the
//! load commands, which sit right after the header and are followed by free
//! space up to the first section's data. So each image (the whole file, or one
//! architecture of a fat file) is kept as a copy of just that prefix, edited in
//! memory, and written back over the original bytes. The rest of the file is
//! never read or copied.

use std::{
    borrow::Cow,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Deref,
    sync::OnceLock,
};

use goblin::{
    container::Ctx,
    mach::{
        constants::{SECTION_TYPE, S_GB_ZEROFILL, S_THREAD_LOCAL_ZEROFILL, S_ZEROFILL},
        fat::{FatArch, FAT_MAGIC, SIZEOF_FAT_ARCH, SIZEOF_FAT_HEADER},
        header::{SIZEOF_HEADER_32, SIZEOF_HEADER_64},
        load_command::{
            DylibCommand, Section32, Section64, SegmentCommand32, SegmentCommand64, LC_ID_DYLIB,
            LC_LAZY_LOAD_DYLIB, LC_LOAD_DYLIB, LC_LOAD_UPWARD_DYLIB, LC_LOAD_WEAK_DYLIB,
            LC_REEXPORT_DYLIB, LC_RPATH, LC_SEGMENT, LC_SEGMENT_64, SIZEOF_SECTION_32,
            SIZEOF_SECTION_64, SIZEOF_SEGMENT_COMMAND_32, SIZEOF_SEGMENT_COMMAND_64,
        },
        parse_magic_and_ctx,
    },
};
use scroll::{Pread, BE};

use crate::{
    commands::{DlibCommandBuilder, RpathCommandBuilder},
    error::MachoError,
};

/// Magic of a static library (`ar` archive), which fat files may contain.
const AR_MAGIC: &[u8; 8] = b"!<arch>\n";

/// Commands that load a dylib, in the order goblin lists them as `libs`.
const DYLIB_LOADS: [u32; 5] = [
    LC_LOAD_DYLIB,
    LC_LOAD_UPWARD_DYLIB,
    LC_REEXPORT_DYLIB,
    LC_LOAD_WEAK_DYLIB,
    LC_LAZY_LOAD_DYLIB,
];

/// Reads `len` bytes at `offset` of the file being parsed.
type ReadAt<'r> = dyn FnMut(u64, usize) -> Result<Vec<u8>, MachoError> + 'r;

/// One Mach-O image: the whole file, or one architecture of a fat file.
#[derive(Clone, Debug)]
struct Image {
    offset: u64,
    ctx: Ctx,
    /// The header, the load commands and the free space after them, as they
    /// are in the file.
    orig: Vec<u8>,
    /// The same bytes with the edits applied. Always as long as `orig`.
    bytes: Vec<u8>,
}

impl Image {
    fn read(read_at: &mut ReadAt, offset: u64, size: u64) -> Result<Self, MachoError> {
        let head = read_at(offset, SIZEOF_HEADER_64.min(size as usize))?;
        if head.len() >= AR_MAGIC.len() && head[..AR_MAGIC.len()] == AR_MAGIC[..] {
            return Err(MachoError::UnixArchive);
        }
        let (magic, ctx) = parse_magic_and_ctx(&head, 0)?;
        let ctx = ctx.ok_or(goblin::error::Error::BadMagic(u64::from(magic)))?;
        let header_size = header_size(ctx);
        if head.len() < header_size {
            return Err(MachoError::Truncated);
        }
        let sizeofcmds: u32 = head.pread_with(20, ctx.le)?;
        let commands_end = header_size + sizeofcmds as usize;
        if commands_end as u64 > size {
            return Err(MachoError::Truncated);
        }
        let commands = read_at(offset, commands_end)?;
        let free_end = free_space_end(&commands, ctx, size)?.max(commands_end);
        let orig = read_at(offset, free_end)?;
        Ok(Image {
            offset,
            ctx,
            bytes: orig.clone(),
            orig,
        })
    }

    fn header_size(&self) -> usize {
        header_size(self.ctx)
    }

    fn ncmds(&self) -> u32 {
        self.bytes.pread_with(16, self.ctx.le).unwrap()
    }

    fn sizeofcmds(&self) -> u32 {
        self.bytes.pread_with(20, self.ctx.le).unwrap()
    }

    fn set_counts(&mut self, ncmds: u32, sizeofcmds: u32) {
        use scroll::Pwrite;
        self.bytes.pwrite_with(ncmds, 16, self.ctx.le).unwrap();
        self.bytes.pwrite_with(sizeofcmds, 20, self.ctx.le).unwrap();
    }

    /// The load commands as `(offset, cmd, cmdsize)`, in file order.
    fn commands(&self) -> Result<Vec<(usize, u32, usize)>, MachoError> {
        load_commands(&self.bytes, self.ctx)
    }

    /// The string a load command stores at `str_offset` from its start.
    fn command_string(&self, at: usize, cmdsize: usize, str_offset: u32) -> &[u8] {
        let start = (at + str_offset as usize).min(at + cmdsize);
        let field = &self.bytes[start..at + cmdsize];
        let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        &field[..len]
    }

    /// Replace `remove` bytes of load commands at `at` with `insert`, moving
    /// the following commands and zeroing the space they leave.
    fn splice_commands(
        &mut self,
        at: usize,
        remove: usize,
        insert: &[u8],
        ncmds: u32,
    ) -> Result<(), MachoError> {
        let end = self.header_size() + self.sizeofcmds() as usize;
        let new_end = end - remove + insert.len();
        if new_end > self.bytes.len() {
            return Err(MachoError::NoSpace {
                needed: new_end - self.header_size(),
                available: self.bytes.len() - self.header_size(),
            });
        }
        let tail = self.bytes[at + remove..end].to_vec();
        self.bytes[at..at + insert.len()].copy_from_slice(insert);
        self.bytes[at + insert.len()..new_end].copy_from_slice(&tail);
        if new_end < end {
            self.bytes[new_end..end].fill(0);
        }
        self.set_counts(ncmds, (new_end - self.header_size()) as u32);
        Ok(())
    }

    /// The first `LC_RPATH` with path `rpath`, as `(offset, cmdsize)`.
    fn find_rpath(&self, rpath: &str) -> Result<Option<(usize, usize)>, MachoError> {
        for (at, cmd, cmdsize) in self.commands()? {
            if cmd != LC_RPATH {
                continue;
            }
            let path: u32 = self.bytes.pread_with(at + 8, self.ctx.le)?;
            if self.command_string(at, cmdsize, path) == rpath.as_bytes() {
                return Ok(Some((at, cmdsize)));
            }
        }
        Ok(None)
    }

    /// The first command of one of the `kinds` whose dylib is called `name`
    /// (any name if `None`), as `(offset, cmdsize, command)`.
    fn find_dylib(
        &self,
        kinds: &[u32],
        name: Option<&str>,
    ) -> Result<Option<(usize, usize, DylibCommand)>, MachoError> {
        for (at, cmd, cmdsize) in self.commands()? {
            if !kinds.contains(&cmd) {
                continue;
            }
            let command: DylibCommand = self.bytes.pread_with(at, self.ctx.le)?;
            let found = self.command_string(at, cmdsize, command.dylib.name);
            if name.is_none_or(|name| found == name.as_bytes()) {
                return Ok(Some((at, cmdsize, command)));
            }
        }
        Ok(None)
    }

    fn add_rpath(&mut self, new_rpath: &str) -> Result<(), MachoError> {
        let (_, command) = RpathCommandBuilder::new(new_rpath, self.ctx).build()?;
        let end = self.header_size() + self.sizeofcmds() as usize;
        self.splice_commands(end, 0, &command, self.ncmds() + 1)
    }

    fn change_rpath(&mut self, old_rpath: &str, new_rpath: &str) -> Result<(), MachoError> {
        let (at, cmdsize) = self
            .find_rpath(old_rpath)?
            .ok_or_else(|| MachoError::RpathMissing(old_rpath.to_string()))?;
        let (_, command) = RpathCommandBuilder::new(new_rpath, self.ctx).build()?;
        self.splice_commands(at, cmdsize, &command, self.ncmds())
    }

    fn remove_rpath(&mut self, old_rpath: &str) -> Result<(), MachoError> {
        let (at, cmdsize) = self
            .find_rpath(old_rpath)?
            .ok_or_else(|| MachoError::RpathMissing(old_rpath.to_string()))?;
        self.splice_commands(at, cmdsize, &[], self.ncmds() - 1)
    }

    fn change_install_id(&mut self, new_id: &str) -> Result<(), MachoError> {
        let (at, cmdsize, old) = self
            .find_dylib(&[LC_ID_DYLIB], None)?
            .ok_or(MachoError::DylibIdMissing)?;
        let (_, command) = DlibCommandBuilder::new(new_id, old, self.ctx).build()?;
        self.splice_commands(at, cmdsize, &command, self.ncmds())
    }

    fn change_install_name(&mut self, old_name: &str, new_name: &str) -> Result<(), MachoError> {
        let (at, cmdsize, old) = self
            .find_dylib(&DYLIB_LOADS, Some(old_name))?
            .ok_or_else(|| MachoError::DylibNameMissing(old_name.to_string()))?;
        let (_, command) = DlibCommandBuilder::new(new_name, old, self.ctx).build()?;
        self.splice_commands(at, cmdsize, &command, self.ncmds())
    }

    /// The range of `bytes` that differs from `orig`, if any.
    fn changed_range(&self) -> Option<std::ops::Range<usize>> {
        let first = self
            .orig
            .iter()
            .zip(&self.bytes)
            .position(|(a, b)| a != b)?;
        let last = self
            .orig
            .iter()
            .zip(&self.bytes)
            .rposition(|(a, b)| a != b)?;
        Some(first..last + 1)
    }
}

fn header_size(ctx: Ctx) -> usize {
    if ctx.container.is_big() {
        SIZEOF_HEADER_64
    } else {
        SIZEOF_HEADER_32
    }
}

/// The load commands in `bytes` (an image's header and commands) as
/// `(offset, cmd, cmdsize)`.
fn load_commands(bytes: &[u8], ctx: Ctx) -> Result<Vec<(usize, u32, usize)>, MachoError> {
    let ncmds: u32 = bytes.pread_with(16, ctx.le)?;
    let sizeofcmds: u32 = bytes.pread_with(20, ctx.le)?;
    let end = header_size(ctx) + sizeofcmds as usize;
    let mut at = header_size(ctx);
    let mut commands = Vec::with_capacity(ncmds as usize);
    for _ in 0..ncmds {
        let cmd: u32 = bytes.pread_with(at, ctx.le)?;
        let cmdsize: u32 = bytes.pread_with(at + 4, ctx.le)?;
        let cmdsize = cmdsize as usize;
        if cmdsize < 8 || at + cmdsize > end {
            return Err(MachoError::BadLoadCommand(at));
        }
        commands.push((at, cmd, cmdsize));
        at += cmdsize;
    }
    Ok(commands)
}

/// Where the free space after the load commands ends: at the lowest file
/// offset that holds section or segment data, or at the end of the image.
fn free_space_end(commands: &[u8], ctx: Ctx, size: u64) -> Result<usize, MachoError> {
    let mut end = size;
    let is_zerofill = |flags: u32| {
        matches!(
            flags & SECTION_TYPE,
            S_ZEROFILL | S_GB_ZEROFILL | S_THREAD_LOCAL_ZEROFILL
        )
    };
    for (at, cmd, _) in load_commands(commands, ctx)? {
        // (file offset, file size, number of sections, first section, section size)
        let (fileoff, filesize, nsects, sections_at, section_size) = match cmd {
            LC_SEGMENT_64 => {
                let s: SegmentCommand64 = commands.pread_with(at, ctx.le)?;
                let first = at + SIZEOF_SEGMENT_COMMAND_64;
                (s.fileoff, s.filesize, s.nsects, first, SIZEOF_SECTION_64)
            }
            LC_SEGMENT => {
                let s: SegmentCommand32 = commands.pread_with(at, ctx.le)?;
                let first = at + SIZEOF_SEGMENT_COMMAND_32;
                let (fileoff, filesize) = (u64::from(s.fileoff), u64::from(s.filesize));
                (fileoff, filesize, s.nsects, first, SIZEOF_SECTION_32)
            }
            _ => continue,
        };
        if nsects == 0 {
            if fileoff != 0 && filesize != 0 {
                end = end.min(fileoff);
            }
            continue;
        }
        for i in 0..nsects as usize {
            let section_at = sections_at + i * section_size;
            let (offset, flags) = if cmd == LC_SEGMENT_64 {
                let s: Section64 = commands.pread_with(section_at, ctx.le)?;
                (s.offset, s.flags)
            } else {
                let s: Section32 = commands.pread_with(section_at, ctx.le)?;
                (s.offset, s.flags)
            };
            if offset != 0 && !is_zerofill(flags) {
                end = end.min(u64::from(offset));
            }
        }
    }
    Ok(end as usize)
}

/// Read the images of a thin or fat file of `len` bytes.
fn read_images(read_at: &mut ReadAt, len: u64) -> Result<Vec<Image>, MachoError> {
    let magic = read_at(0, 4.min(len as usize))?;
    let magic: u32 = magic.pread_with(0, BE)?;
    if magic != FAT_MAGIC {
        return Ok(vec![Image::read(read_at, 0, len)?]);
    }
    let header = read_at(0, SIZEOF_FAT_HEADER)?;
    let nfat_arch: u32 = header.pread_with(4, BE)?;
    let arches = read_at(
        SIZEOF_FAT_HEADER as u64,
        nfat_arch as usize * SIZEOF_FAT_ARCH,
    )?;
    let mut images = Vec::with_capacity(nfat_arch as usize);
    for i in 0..nfat_arch as usize {
        let arch: FatArch = arches.pread_with(i * SIZEOF_FAT_ARCH, BE)?;
        let (offset, size) = (u64::from(arch.offset), u64::from(arch.size));
        if offset + size > len {
            return Err(MachoError::FatArch);
        }
        images.push(Image::read(read_at, offset, size)?);
    }
    Ok(images)
}

/// The images of a Mach-O file with the edits made so far.
///
/// Every edit applies to all architectures of a fat file, and either succeeds
/// for all of them or changes none.
#[derive(Clone, Debug)]
struct Images(Vec<Image>);

impl Images {
    fn edit(
        &mut self,
        edit: impl Fn(&mut Image) -> Result<(), MachoError>,
    ) -> Result<(), MachoError> {
        let mut edited = self.0.clone();
        for image in &mut edited {
            edit(image)?;
        }
        self.0 = edited;
        Ok(())
    }

    /// The changed bytes of every image, as `(file offset, bytes)`.
    fn changes(&self) -> impl Iterator<Item = (u64, &[u8])> {
        self.0.iter().filter_map(|image| {
            let range = image.changed_range()?;
            Some((image.offset + range.start as u64, &image.bytes[range]))
        })
    }

    /// Mark the current bytes as written.
    fn synced(&mut self) {
        for image in &mut self.0 {
            image.orig.clone_from(&image.bytes);
        }
    }
}

/// The edit operations, shared by [`MachoContainer`] and [`MachoFile`].
macro_rules! edit_operations {
    ($($after:ident)?) => {
        /// Adds a new rpath.
        pub fn add_rpath(&mut self, new_rpath: &str) -> Result<(), MachoError> {
            self.images.edit(|image| image.add_rpath(new_rpath))?;
            $(self.$after();)?
            Ok(())
        }

        /// Changes the first rpath `old_rpath` to `new_rpath`.
        pub fn change_rpath(&mut self, old_rpath: &str, new_rpath: &str) -> Result<(), MachoError> {
            self.images.edit(|image| image.change_rpath(old_rpath, new_rpath))?;
            $(self.$after();)?
            Ok(())
        }

        /// Removes the first rpath `old_rpath`.
        pub fn remove_rpath(&mut self, old_rpath: &str) -> Result<(), MachoError> {
            self.images.edit(|image| image.remove_rpath(old_rpath))?;
            $(self.$after();)?
            Ok(())
        }

        /// Changes the install id (`LC_ID_DYLIB`) of a dylib.
        pub fn change_install_id(&mut self, new_id: &str) -> Result<(), MachoError> {
            self.images.edit(|image| image.change_install_id(new_id))?;
            $(self.$after();)?
            Ok(())
        }

        /// Changes the install name of the dependency `old_name` to `new_name`.
        pub fn change_install_name(
            &mut self,
            old_name: &str,
            new_name: &str,
        ) -> Result<(), MachoError> {
            self.images
                .edit(|image| image.change_install_name(old_name, new_name))?;
            $(self.$after();)?
            Ok(())
        }
    };
}

/// A Mach-O file held in memory.
pub struct MachoContainer<'a> {
    images: Images,

    /// The bytes of the file with all edits applied.
    pub data: PatchedBytes<'a>,
}

impl<'a> MachoContainer<'a> {
    /// Parse a thin or fat Mach-O file.
    ///
    /// Borrowed data is never copied as a whole; the full patched file is only
    /// assembled when [`MachoContainer::data`] is read.
    pub fn parse(bytes: impl Into<Cow<'a, [u8]>>) -> Result<Self, MachoError> {
        let base = bytes.into();
        let images = {
            let mut read_at = |offset: u64, len: usize| -> Result<Vec<u8>, MachoError> {
                let start = usize::try_from(offset).map_err(|_| MachoError::Truncated)?;
                base.get(start..start + len)
                    .map(<[u8]>::to_vec)
                    .ok_or(MachoError::Truncated)
            };
            read_images(&mut read_at, base.len() as u64)?
        };
        Ok(MachoContainer {
            images: Images(images),
            data: PatchedBytes {
                base,
                patches: Vec::new(),
                full: OnceLock::new(),
            },
        })
    }

    edit_operations!(refresh_data);

    fn refresh_data(&mut self) {
        self.data.patches = self
            .images
            .changes()
            .map(|(offset, bytes)| (offset as usize, bytes.to_vec()))
            .collect();
        self.data.full = OnceLock::new();
    }
}

/// The bytes of a [`MachoContainer`]'s file with its edits applied, assembled
/// on first access.
pub struct PatchedBytes<'a> {
    base: Cow<'a, [u8]>,
    /// Bytes to put over `base`, as `(offset, bytes)`.
    patches: Vec<(usize, Vec<u8>)>,
    full: OnceLock<Vec<u8>>,
}

impl Deref for PatchedBytes<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        if self.patches.is_empty() {
            return &self.base;
        }
        self.full.get_or_init(|| {
            let mut full = self.base.to_vec();
            for (offset, bytes) in &self.patches {
                full[*offset..*offset + bytes.len()].copy_from_slice(bytes);
            }
            full
        })
    }
}

impl AsRef<[u8]> for PatchedBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

/// A Mach-O file edited in place: only the headers and load commands are read,
/// and only the bytes that changed are written back.
pub struct MachoFile {
    images: Images,
    /// The length of the file when it was read.
    len: u64,
}

impl MachoFile {
    /// Read the headers and load commands of the thin or fat Mach-O `file`.
    pub fn read(file: &mut File) -> Result<Self, MachoError> {
        let len = file.metadata()?.len();
        let mut read_at = |offset: u64, n: usize| -> Result<Vec<u8>, MachoError> {
            let mut buf = vec![0; n];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut buf).map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => MachoError::Truncated,
                _ => MachoError::Io(e),
            })?;
            Ok(buf)
        };
        let images = read_images(&mut read_at, len)?;
        Ok(MachoFile {
            images: Images(images),
            len,
        })
    }

    edit_operations!();

    /// Write the changed bytes back to `file`, which must be the file this was
    /// read from, unchanged since. Everything is laid out before `file` is
    /// written to, so an edit that fails leaves it untouched.
    pub fn write_changes(&mut self, file: &mut File) -> Result<(), MachoError> {
        let on_disk = file.metadata()?.len();
        if on_disk != self.len {
            return Err(MachoError::FileChanged {
                read: self.len,
                now: on_disk,
            });
        }
        for (offset, bytes) in self.images.changes() {
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(bytes)?;
        }
        file.flush()?;
        self.images.synced();
        Ok(())
    }
}
