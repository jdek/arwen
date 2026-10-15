//! Parsing the headers of an ELF file.

use super::codec::{slice, Codec, FileHeader, ProgramHeader, SectionHeader};
use super::consts::*;
use super::{ElfError, Result};

/// The headers of an ELF executable or shared object.
#[derive(Debug, Clone)]
pub(crate) struct ElfFile {
    pub codec: Codec,
    pub header: FileHeader,
    pub phdrs: Vec<ProgramHeader>,
    pub shdrs: Vec<SectionHeader>,
}

impl ElfFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let ident = slice(data, 0, 16, "ELF identification")?;
        if ident[..4] != ELFMAG {
            return Err(ElfError::Malformed("not an ELF file".to_string()));
        }
        let is_64 = match ident[EI_CLASS] {
            ELFCLASS32 => false,
            ELFCLASS64 => true,
            class => {
                return Err(ElfError::Malformed(format!("unknown ELF class {class}")));
            }
        };
        let little_endian = match ident[EI_DATA] {
            ELFDATA2LSB => true,
            ELFDATA2MSB => false,
            order => {
                return Err(ElfError::Malformed(format!(
                    "unknown ELF byte order {order}"
                )));
            }
        };
        let codec = Codec {
            is_64,
            little_endian,
        };
        let header_bytes = slice(data, 0, codec.file_header_size() as u64, "file header")?;
        let header = FileHeader::read(codec, header_bytes);

        match header.e_type {
            ET_EXEC | ET_DYN => {}
            ET_REL => {
                return Err(ElfError::Unsupported(
                    "relocatable object files (ET_REL) have no dynamic section to patch".into(),
                ))
            }
            ET_CORE => {
                return Err(ElfError::Unsupported(
                    "core dumps (ET_CORE) cannot be patched".into(),
                ))
            }
            other => {
                return Err(ElfError::Unsupported(format!(
                    "unsupported ELF file type {other}"
                )))
            }
        }

        let mut phdrs = Vec::with_capacity(usize::from(header.e_phnum));
        if header.e_phnum > 0 {
            if usize::from(header.e_phentsize) != codec.program_header_size() {
                return Err(ElfError::Malformed(format!(
                    "unexpected program header size {}",
                    header.e_phentsize
                )));
            }
            let size = codec.program_header_size() as u64;
            for i in 0..u64::from(header.e_phnum) {
                let b = slice(data, header.e_phoff + i * size, size, "program header")?;
                phdrs.push(ProgramHeader::read(codec, b));
            }
        }

        let mut shdrs = Vec::new();
        if header.e_shoff != 0 {
            if usize::from(header.e_shentsize) != codec.section_header_size() {
                return Err(ElfError::Malformed(format!(
                    "unexpected section header size {}",
                    header.e_shentsize
                )));
            }
            let size = codec.section_header_size() as u64;
            let first =
                SectionHeader::read(codec, slice(data, header.e_shoff, size, "section header")?);
            // With more than SHN_LORESERVE sections, the count is stored in the
            // first section header.
            let count = if header.e_shnum == 0 {
                first.sh_size
            } else {
                u64::from(header.e_shnum)
            };
            shdrs.reserve(count as usize);
            for i in 0..count {
                let b = slice(data, header.e_shoff + i * size, size, "section header")?;
                shdrs.push(SectionHeader::read(codec, b));
            }
        }
        if shdrs.is_empty() {
            return Err(ElfError::Unsupported(
                "files without section headers cannot be patched".into(),
            ));
        }

        let file = Self {
            codec,
            header,
            phdrs,
            shdrs,
        };
        for (i, sh) in file.shdrs.iter().enumerate() {
            if sh.has_file_data() {
                slice(data, sh.sh_offset, sh.sh_size, "section contents").map_err(|_| {
                    ElfError::Malformed(format!("section {i} extends past the end of the file"))
                })?;
            }
        }
        Ok(file)
    }

    pub fn section_data<'a>(&self, data: &'a [u8], sh: &SectionHeader) -> Result<&'a [u8]> {
        if !sh.has_file_data() {
            return Ok(&[]);
        }
        slice(data, sh.sh_offset, sh.sh_size, "section contents")
    }

    pub fn section_by_type(&self, sh_type: u32) -> Option<usize> {
        self.shdrs.iter().position(|sh| sh.sh_type == sh_type)
    }

    /// The allocated section whose file data starts at `offset`.
    pub fn section_at_offset(&self, offset: u64, sh_type: Option<u32>) -> Option<usize> {
        self.shdrs.iter().position(|sh| {
            sh.is_alloc()
                && sh.has_file_data()
                && sh.sh_offset == offset
                && sh_type.is_none_or(|t| sh.sh_type == t)
        })
    }

    /// The allocated section starting at virtual address `addr`.
    pub fn section_at_address(&self, addr: u64) -> Option<usize> {
        self.shdrs
            .iter()
            .position(|sh| sh.is_alloc() && sh.sh_addr == addr && sh.sh_size > 0)
    }

    pub fn segment(&self, p_type: u32) -> Option<&ProgramHeader> {
        self.phdrs.iter().find(|ph| ph.p_type == p_type)
    }

    /// The section holding the dynamic table, found through `PT_DYNAMIC`.
    pub fn dynamic_section(&self) -> Option<usize> {
        let segment = self.segment(PT_DYNAMIC)?;
        self.section_at_offset(segment.p_offset, Some(SHT_DYNAMIC))
            .or_else(|| self.section_by_type(SHT_DYNAMIC))
    }

    /// The section holding the interpreter, found through `PT_INTERP`.
    pub fn interp_section(&self) -> Option<usize> {
        let segment = self.segment(PT_INTERP)?;
        self.section_at_offset(segment.p_offset, None)
    }
}
