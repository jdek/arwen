//! ELF constants used by the rewriter, from the System V gABI and the GNU extensions.

#![allow(missing_docs)]

// e_ident
pub const ELFMAG: [u8; 4] = *b"\x7fELF";
pub const EI_CLASS: usize = 4;
pub const EI_DATA: usize = 5;
pub const EI_OSABI: usize = 7;
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFDATA2MSB: u8 = 2;

// e_type
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const ET_CORE: u16 = 4;

// e_machine
pub const EM_SPARC: u16 = 2;
pub const EM_386: u16 = 3;
pub const EM_MIPS: u16 = 8;
pub const EM_PPC: u16 = 20;
pub const EM_PPC64: u16 = 21;
pub const EM_SPARCV9: u16 = 43;
pub const EM_IA_64: u16 = 50;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;
pub const EM_TILEGX: u16 = 191;
pub const EM_LOONGARCH: u16 = 258;
pub const EM_ALPHA: u16 = 0x9026;

// EI_OSABI
pub const ELFOSABI_SYSV: u8 = 0;
pub const ELFOSABI_HPUX: u8 = 1;
pub const ELFOSABI_NETBSD: u8 = 2;
pub const ELFOSABI_LINUX: u8 = 3;
pub const ELFOSABI_HURD: u8 = 4;
pub const ELFOSABI_SOLARIS: u8 = 6;
pub const ELFOSABI_AIX: u8 = 7;
pub const ELFOSABI_IRIX: u8 = 8;
pub const ELFOSABI_FREEBSD: u8 = 9;
pub const ELFOSABI_TRU64: u8 = 10;
pub const ELFOSABI_MODESTO: u8 = 11;
pub const ELFOSABI_OPENBSD: u8 = 12;
pub const ELFOSABI_OPENVMS: u8 = 13;
pub const ELFOSABI_NSK: u8 = 14;
pub const ELFOSABI_AROS: u8 = 15;
pub const ELFOSABI_FENIXOS: u8 = 16;
pub const ELFOSABI_CLOUDABI: u8 = 17;

// p_type
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;
pub const PT_GNU_PROPERTY: u32 = 0x6474_e553;

// p_flags
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// sh_type
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_HASH: u32 = 5;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;
pub const SHT_RELR: u32 = 19;
pub const SHT_GNU_HASH: u32 = 0x6fff_fff6;
pub const SHT_GNU_VERDEF: u32 = 0x6fff_fffd;
pub const SHT_GNU_VERNEED: u32 = 0x6fff_fffe;
pub const SHT_GNU_VERSYM: u32 = 0x6fff_ffff;

// sh_flags
pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;

// Special section indices
pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;
pub const SHN_XINDEX: u16 = 0xffff;

// d_tag
pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SONAME: i64 = 14;
pub const DT_RPATH: i64 = 15;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_JMPREL: i64 = 23;
pub const DT_RUNPATH: i64 = 29;
pub const DT_FLAGS: i64 = 30;
pub const DT_RELRSZ: i64 = 35;
pub const DT_RELR: i64 = 36;
pub const DT_ANDROID_REL: i64 = 0x6000_000f;
pub const DT_ANDROID_RELA: i64 = 0x6000_0011;
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_CONFIG: i64 = 0x6fff_fefa;
pub const DT_DEPAUDIT: i64 = 0x6fff_fefb;
pub const DT_AUDIT: i64 = 0x6fff_fefc;
pub const DT_SYMINFO: i64 = 0x6fff_feff;
pub const DT_VERSYM: i64 = 0x6fff_fff0;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;
pub const DT_VERDEF: i64 = 0x6fff_fffc;
pub const DT_VERDEFNUM: i64 = 0x6fff_fffd;
pub const DT_VERNEED: i64 = 0x6fff_fffe;
pub const DT_VERNEEDNUM: i64 = 0x6fff_ffff;
pub const DT_AUXILIARY: i64 = 0x7fff_fffd;
pub const DT_FILTER: i64 = 0x7fff_ffff;
pub const DT_MIPS_RLD_MAP_REL: i64 = 0x7000_0035;

// DT_FLAGS_1
pub const DF_1_NODEFLIB: u64 = 0x800;

// Symbol versions
pub const VER_NDX_GLOBAL: u16 = 1;
