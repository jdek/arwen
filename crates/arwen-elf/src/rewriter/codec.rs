//! Reading and writing ELF structures for both classes and both byte orders.
//!
//! All structures are decoded into class-independent forms with 64-bit fields
//! and encoded back to the file's own class and byte order.

use super::{ElfError, Result};

/// The class and byte order of an ELF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Codec {
    pub is_64: bool,
    pub little_endian: bool,
}

fn truncated(what: &str, offset: u64) -> ElfError {
    ElfError::Malformed(format!("{what} at offset {offset:#x} is outside the file"))
}

/// Get `len` bytes at `offset`, failing instead of panicking on a malformed file.
pub(crate) fn slice<'a>(data: &'a [u8], offset: u64, len: u64, what: &str) -> Result<&'a [u8]> {
    let start = usize::try_from(offset).map_err(|_| truncated(what, offset))?;
    let len = usize::try_from(len).map_err(|_| truncated(what, offset))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| truncated(what, offset))?;
    data.get(start..end).ok_or_else(|| truncated(what, offset))
}

impl Codec {
    pub fn u16(&self, b: &[u8], at: usize) -> u16 {
        let raw = [b[at], b[at + 1]];
        if self.little_endian {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        }
    }

    pub fn u32(&self, b: &[u8], at: usize) -> u32 {
        let raw: [u8; 4] = b[at..at + 4].try_into().unwrap();
        if self.little_endian {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        }
    }

    pub fn u64(&self, b: &[u8], at: usize) -> u64 {
        let raw: [u8; 8] = b[at..at + 8].try_into().unwrap();
        if self.little_endian {
            u64::from_le_bytes(raw)
        } else {
            u64::from_be_bytes(raw)
        }
    }

    /// A class-sized word: 4 bytes for ELF32, 8 bytes for ELF64.
    pub fn word(&self, b: &[u8], at: usize) -> u64 {
        if self.is_64 {
            self.u64(b, at)
        } else {
            u64::from(self.u32(b, at))
        }
    }

    pub fn put_u16(&self, b: &mut [u8], at: usize, v: u16) {
        let raw = if self.little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        b[at..at + 2].copy_from_slice(&raw);
    }

    pub fn put_u32(&self, b: &mut [u8], at: usize, v: u32) {
        let raw = if self.little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        b[at..at + 4].copy_from_slice(&raw);
    }

    pub fn put_u64(&self, b: &mut [u8], at: usize, v: u64) {
        let raw = if self.little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        b[at..at + 8].copy_from_slice(&raw);
    }

    /// Write a class-sized word. On ELF32 the value must fit in 32 bits.
    pub fn put_word(&self, b: &mut [u8], at: usize, v: u64) {
        if self.is_64 {
            self.put_u64(b, at, v)
        } else {
            debug_assert!(v <= u64::from(u32::MAX));
            self.put_u32(b, at, v as u32)
        }
    }

    pub fn file_header_size(&self) -> usize {
        if self.is_64 {
            64
        } else {
            52
        }
    }

    pub fn program_header_size(&self) -> usize {
        if self.is_64 {
            56
        } else {
            32
        }
    }

    pub fn section_header_size(&self) -> usize {
        if self.is_64 {
            64
        } else {
            40
        }
    }

    pub fn sym_size(&self) -> usize {
        if self.is_64 {
            24
        } else {
            16
        }
    }

    pub fn dyn_size(&self) -> usize {
        if self.is_64 {
            16
        } else {
            8
        }
    }

    pub fn word_size(&self) -> usize {
        if self.is_64 {
            8
        } else {
            4
        }
    }

    /// The largest address or offset the class can represent.
    pub fn max_address(&self) -> u64 {
        if self.is_64 {
            u64::MAX
        } else {
            u64::from(u32::MAX)
        }
    }
}

/// The file header, without `e_ident` apart from the OS/ABI bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHeader {
    pub os_abi: u8,
    pub e_type: u16,
    pub e_machine: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

impl FileHeader {
    pub fn read(c: Codec, b: &[u8]) -> Self {
        let os_abi = b[7];
        if c.is_64 {
            Self {
                os_abi,
                e_type: c.u16(b, 16),
                e_machine: c.u16(b, 18),
                e_entry: c.u64(b, 24),
                e_phoff: c.u64(b, 32),
                e_shoff: c.u64(b, 40),
                e_flags: c.u32(b, 48),
                e_phentsize: c.u16(b, 54),
                e_phnum: c.u16(b, 56),
                e_shentsize: c.u16(b, 58),
                e_shnum: c.u16(b, 60),
                e_shstrndx: c.u16(b, 62),
            }
        } else {
            Self {
                os_abi,
                e_type: c.u16(b, 16),
                e_machine: c.u16(b, 18),
                e_entry: u64::from(c.u32(b, 24)),
                e_phoff: u64::from(c.u32(b, 28)),
                e_shoff: u64::from(c.u32(b, 32)),
                e_flags: c.u32(b, 36),
                e_phentsize: c.u16(b, 42),
                e_phnum: c.u16(b, 44),
                e_shentsize: c.u16(b, 46),
                e_shnum: c.u16(b, 48),
                e_shstrndx: c.u16(b, 50),
            }
        }
    }

    /// Write the fields this rewriter changes. Everything else is left as it is.
    pub fn write(&self, c: Codec, b: &mut [u8]) {
        b[7] = self.os_abi;
        if c.is_64 {
            c.put_u64(b, 32, self.e_phoff);
            c.put_u64(b, 40, self.e_shoff);
            c.put_u16(b, 56, self.e_phnum);
        } else {
            c.put_u32(b, 28, self.e_phoff as u32);
            c.put_u32(b, 32, self.e_shoff as u32);
            c.put_u16(b, 44, self.e_phnum);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

impl ProgramHeader {
    pub fn read(c: Codec, b: &[u8]) -> Self {
        if c.is_64 {
            Self {
                p_type: c.u32(b, 0),
                p_flags: c.u32(b, 4),
                p_offset: c.u64(b, 8),
                p_vaddr: c.u64(b, 16),
                p_paddr: c.u64(b, 24),
                p_filesz: c.u64(b, 32),
                p_memsz: c.u64(b, 40),
                p_align: c.u64(b, 48),
            }
        } else {
            Self {
                p_type: c.u32(b, 0),
                p_offset: u64::from(c.u32(b, 4)),
                p_vaddr: u64::from(c.u32(b, 8)),
                p_paddr: u64::from(c.u32(b, 12)),
                p_filesz: u64::from(c.u32(b, 16)),
                p_memsz: u64::from(c.u32(b, 20)),
                p_flags: c.u32(b, 24),
                p_align: u64::from(c.u32(b, 28)),
            }
        }
    }

    pub fn write(&self, c: Codec, b: &mut [u8]) {
        if c.is_64 {
            c.put_u32(b, 0, self.p_type);
            c.put_u32(b, 4, self.p_flags);
            c.put_u64(b, 8, self.p_offset);
            c.put_u64(b, 16, self.p_vaddr);
            c.put_u64(b, 24, self.p_paddr);
            c.put_u64(b, 32, self.p_filesz);
            c.put_u64(b, 40, self.p_memsz);
            c.put_u64(b, 48, self.p_align);
        } else {
            c.put_u32(b, 0, self.p_type);
            c.put_u32(b, 4, self.p_offset as u32);
            c.put_u32(b, 8, self.p_vaddr as u32);
            c.put_u32(b, 12, self.p_paddr as u32);
            c.put_u32(b, 16, self.p_filesz as u32);
            c.put_u32(b, 20, self.p_memsz as u32);
            c.put_u32(b, 24, self.p_flags);
            c.put_u32(b, 28, self.p_align as u32);
        }
    }

    pub fn file_end(&self) -> u64 {
        self.p_offset + self.p_filesz
    }

    pub fn mem_end(&self) -> u64 {
        self.p_vaddr + self.p_memsz
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SectionHeader {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
}

impl SectionHeader {
    pub fn read(c: Codec, b: &[u8]) -> Self {
        if c.is_64 {
            Self {
                sh_name: c.u32(b, 0),
                sh_type: c.u32(b, 4),
                sh_flags: c.u64(b, 8),
                sh_addr: c.u64(b, 16),
                sh_offset: c.u64(b, 24),
                sh_size: c.u64(b, 32),
                sh_link: c.u32(b, 40),
                sh_info: c.u32(b, 44),
                sh_addralign: c.u64(b, 48),
                sh_entsize: c.u64(b, 56),
            }
        } else {
            Self {
                sh_name: c.u32(b, 0),
                sh_type: c.u32(b, 4),
                sh_flags: u64::from(c.u32(b, 8)),
                sh_addr: u64::from(c.u32(b, 12)),
                sh_offset: u64::from(c.u32(b, 16)),
                sh_size: u64::from(c.u32(b, 20)),
                sh_link: c.u32(b, 24),
                sh_info: c.u32(b, 28),
                sh_addralign: u64::from(c.u32(b, 32)),
                sh_entsize: u64::from(c.u32(b, 36)),
            }
        }
    }

    pub fn write(&self, c: Codec, b: &mut [u8]) {
        if c.is_64 {
            c.put_u32(b, 0, self.sh_name);
            c.put_u32(b, 4, self.sh_type);
            c.put_u64(b, 8, self.sh_flags);
            c.put_u64(b, 16, self.sh_addr);
            c.put_u64(b, 24, self.sh_offset);
            c.put_u64(b, 32, self.sh_size);
            c.put_u32(b, 40, self.sh_link);
            c.put_u32(b, 44, self.sh_info);
            c.put_u64(b, 48, self.sh_addralign);
            c.put_u64(b, 56, self.sh_entsize);
        } else {
            c.put_u32(b, 0, self.sh_name);
            c.put_u32(b, 4, self.sh_type);
            c.put_u32(b, 8, self.sh_flags as u32);
            c.put_u32(b, 12, self.sh_addr as u32);
            c.put_u32(b, 16, self.sh_offset as u32);
            c.put_u32(b, 20, self.sh_size as u32);
            c.put_u32(b, 24, self.sh_link);
            c.put_u32(b, 28, self.sh_info);
            c.put_u32(b, 32, self.sh_addralign as u32);
            c.put_u32(b, 36, self.sh_entsize as u32);
        }
    }

    pub fn has_file_data(&self) -> bool {
        self.sh_type != super::consts::SHT_NOBITS && self.sh_type != super::consts::SHT_NULL
    }

    pub fn is_alloc(&self) -> bool {
        self.sh_flags & super::consts::SHF_ALLOC != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sym {
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

impl Sym {
    pub fn read(c: Codec, b: &[u8]) -> Self {
        if c.is_64 {
            Self {
                st_name: c.u32(b, 0),
                st_info: b[4],
                st_other: b[5],
                st_shndx: c.u16(b, 6),
                st_value: c.u64(b, 8),
                st_size: c.u64(b, 16),
            }
        } else {
            Self {
                st_name: c.u32(b, 0),
                st_value: u64::from(c.u32(b, 4)),
                st_size: u64::from(c.u32(b, 8)),
                st_info: b[12],
                st_other: b[13],
                st_shndx: c.u16(b, 14),
            }
        }
    }

    pub fn write(&self, c: Codec, b: &mut [u8]) {
        if c.is_64 {
            c.put_u32(b, 0, self.st_name);
            b[4] = self.st_info;
            b[5] = self.st_other;
            c.put_u16(b, 6, self.st_shndx);
            c.put_u64(b, 8, self.st_value);
            c.put_u64(b, 16, self.st_size);
        } else {
            c.put_u32(b, 0, self.st_name);
            c.put_u32(b, 4, self.st_value as u32);
            c.put_u32(b, 8, self.st_size as u32);
            b[12] = self.st_info;
            b[13] = self.st_other;
            c.put_u16(b, 14, self.st_shndx);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dyn {
    pub tag: i64,
    pub val: u64,
}

impl Dyn {
    pub fn read(c: Codec, b: &[u8]) -> Self {
        if c.is_64 {
            Self {
                tag: c.u64(b, 0) as i64,
                val: c.u64(b, 8),
            }
        } else {
            Self {
                tag: i64::from(c.u32(b, 0) as i32),
                val: u64::from(c.u32(b, 4)),
            }
        }
    }

    pub fn write(&self, c: Codec, b: &mut [u8]) {
        if c.is_64 {
            c.put_u64(b, 0, self.tag as u64);
            c.put_u64(b, 8, self.val);
        } else {
            c.put_u32(b, 0, self.tag as i32 as u32);
            c.put_u32(b, 4, self.val as u32);
        }
    }
}

/// Symbol index of a relocation's `r_info`.
pub(crate) fn reloc_sym(c: Codec, info: u64) -> u64 {
    if c.is_64 {
        info >> 32
    } else {
        info >> 8
    }
}

/// `r_info` with the symbol index replaced.
pub(crate) fn reloc_with_sym(c: Codec, info: u64, sym: u64) -> u64 {
    if c.is_64 {
        (sym << 32) | (info & 0xffff_ffff)
    } else {
        (sym << 8) | (info & 0xff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODECS: [Codec; 4] = [
        Codec {
            is_64: true,
            little_endian: true,
        },
        Codec {
            is_64: true,
            little_endian: false,
        },
        Codec {
            is_64: false,
            little_endian: true,
        },
        Codec {
            is_64: false,
            little_endian: false,
        },
    ];

    #[test]
    fn integers_round_trip_in_both_byte_orders() {
        let le = Codec {
            is_64: true,
            little_endian: true,
        };
        let be = Codec {
            is_64: true,
            little_endian: false,
        };
        let mut b = [0u8; 8];
        le.put_u32(&mut b, 0, 0x1122_3344);
        assert_eq!(&b[..4], &[0x44, 0x33, 0x22, 0x11]);
        be.put_u32(&mut b, 0, 0x1122_3344);
        assert_eq!(&b[..4], &[0x11, 0x22, 0x33, 0x44]);
        be.put_u64(&mut b, 0, 0x0102_0304_0506_0708);
        assert_eq!(b, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(le.u64(&b, 0), 0x0807_0605_0403_0201);
        be.put_u16(&mut b, 0, 0xabcd);
        assert_eq!(be.u16(&b, 0), 0xabcd);
        assert_eq!(le.u16(&b, 0), 0xcdab);
    }

    #[test]
    fn structures_round_trip_for_every_class_and_byte_order() {
        for c in CODECS {
            let max = if c.is_64 { u64::MAX - 7 } else { 0xffff_fff0 };

            let ph = ProgramHeader {
                p_type: 0x6474_e551,
                p_flags: 7,
                p_offset: max - 1,
                p_vaddr: max - 2,
                p_paddr: max - 3,
                p_filesz: max - 4,
                p_memsz: max - 5,
                p_align: 0x1000,
            };
            let mut b = vec![0; c.program_header_size()];
            ph.write(c, &mut b);
            assert_eq!(ProgramHeader::read(c, &b), ph, "{c:?}");

            let sh = SectionHeader {
                sh_name: 0x1234_5678,
                sh_type: 0x6fff_fff6,
                sh_flags: 2,
                sh_addr: max - 1,
                sh_offset: max - 2,
                sh_size: max - 3,
                sh_link: 9,
                sh_info: 10,
                sh_addralign: 8,
                sh_entsize: 24,
            };
            let mut b = vec![0; c.section_header_size()];
            sh.write(c, &mut b);
            assert_eq!(SectionHeader::read(c, &b), sh, "{c:?}");

            let sym = Sym {
                st_name: 77,
                st_info: 0x12,
                st_other: 3,
                st_shndx: 0xfff1,
                st_value: max - 9,
                st_size: 42,
            };
            let mut b = vec![0; c.sym_size()];
            sym.write(c, &mut b);
            assert_eq!(Sym::read(c, &b), sym, "{c:?}");

            for tag in [1, 0x6fff_fef5, 0x7fff_ffff] {
                let d = Dyn { tag, val: max - 11 };
                let mut b = vec![0; c.dyn_size()];
                d.write(c, &mut b);
                assert_eq!(Dyn::read(c, &b), d, "{c:?}");
            }

            let info = reloc_with_sym(c, 0x0000_0007, 1234);
            assert_eq!(reloc_sym(c, info), 1234);
            assert_eq!(reloc_with_sym(c, info, 5) & 0xff, 7);
        }
    }

    #[test]
    fn slice_rejects_out_of_range_reads() {
        let data = [0u8; 16];
        assert!(slice(&data, 8, 8, "x").is_ok());
        assert!(slice(&data, 8, 9, "x").is_err());
        assert!(slice(&data, u64::MAX, 2, "x").is_err());
    }
}
