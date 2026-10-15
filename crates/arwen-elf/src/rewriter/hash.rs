//! Rebuilding the SysV (`.hash`) and GNU (`.gnu.hash`) symbol hash tables.

use super::codec::Codec;
use super::{ElfError, Result};

pub(crate) fn sysv_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

pub(crate) fn gnu_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 5381;
    for &c in name {
        h = h.wrapping_mul(33).wrapping_add(u32::from(c));
    }
    h
}

fn malformed(what: &str) -> ElfError {
    ElfError::Malformed(format!("malformed {what}"))
}

/// Rebuild a SysV hash table in place for symbols with the [`sysv_hash`]es
/// `hashes`, keeping its bucket count.
///
/// `entry_size` is the table's word size: 4 on most targets, 8 on a few 64-bit ones.
pub(crate) fn rebuild_sysv(
    c: Codec,
    entry_size: usize,
    table: &mut [u8],
    hashes: &[u32],
) -> Result<()> {
    let read = |b: &[u8], i: usize| -> u64 {
        if entry_size == 8 {
            c.u64(b, i * 8)
        } else {
            u64::from(c.u32(b, i * 4))
        }
    };
    if table.len() < 2 * entry_size {
        return Err(malformed(".hash"));
    }
    let nbucket = read(table, 0) as usize;
    let nchain = read(table, 1) as usize;
    if nbucket == 0 || nchain != hashes.len() || table.len() < (2 + nbucket + nchain) * entry_size {
        return Err(malformed(".hash"));
    }

    let mut bucket = vec![0u64; nbucket];
    let mut chain = vec![0u64; nchain];
    // Symbol 0 is the null symbol and is never hashed.
    for (i, &h) in hashes.iter().enumerate().skip(1) {
        let b = h as usize % nbucket;
        chain[i] = bucket[b];
        bucket[b] = i as u64;
    }

    let mut put = |i: usize, v: u64| {
        if entry_size == 8 {
            c.put_u64(table, i * 8, v);
        } else {
            c.put_u32(table, i * 4, v as u32);
        }
    };
    for (i, &v) in bucket.iter().chain(&chain).enumerate() {
        put(2 + i, v);
    }
    Ok(())
}

/// The parameters stored at the start of a GNU hash table.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GnuHashHeader {
    pub nbuckets: u32,
    pub symoffset: u32,
    pub bloom_size: u32,
    pub bloom_shift: u32,
}

impl GnuHashHeader {
    pub fn read(c: Codec, table: &[u8]) -> Result<Self> {
        if table.len() < 16 {
            return Err(malformed(".gnu.hash"));
        }
        let header = Self {
            nbuckets: c.u32(table, 0),
            symoffset: c.u32(table, 4),
            bloom_size: c.u32(table, 8),
            bloom_shift: c.u32(table, 12),
        };
        if header.nbuckets == 0 || header.bloom_size == 0 {
            return Err(malformed(".gnu.hash"));
        }
        Ok(header)
    }

    /// The end of the range of `nsyms` dynamic symbols that the table hashes.
    ///
    /// The table hashes the symbols from `symoffset` on, as many as its chain
    /// array holds. Linkers do not always size the chain array to the end of the
    /// symbol table: with no defined symbols, it can be empty although
    /// undefined symbols follow `symoffset`.
    pub fn hashed_end(&self, c: Codec, table_len: usize, nsyms: usize) -> Result<usize> {
        let chains_at = 16 + self.bloom_size as usize * c.word_size() + self.nbuckets as usize * 4;
        let chains = table_len
            .checked_sub(chains_at)
            .ok_or_else(|| malformed(".gnu.hash"))?
            / 4;
        let start = (self.symoffset as usize).min(nsyms);
        Ok(nsyms.min(start + chains))
    }
}

/// The symbol order a GNU hash table requires, for symbols with the
/// [`gnu_hash`]es `hashes`: symbols from `symoffset` on are grouped by bucket,
/// in bucket order. Returns, for each new position, the current index of the
/// symbol to place there.
pub(crate) fn gnu_symbol_order(header: GnuHashHeader, hashes: &[u32]) -> Vec<usize> {
    let start = (header.symoffset as usize).min(hashes.len());
    let mut order: Vec<usize> = (0..hashes.len()).collect();
    // A stable sort keeps the existing order within each bucket.
    order[start..].sort_by_key(|&i| hashes[i] % header.nbuckets);
    order
}

/// Rebuild a GNU hash table in place for symbols with the [`gnu_hash`]es
/// `hashes`, which must already be in [`gnu_symbol_order`]. The table keeps
/// its parameters and therefore its size.
pub(crate) fn rebuild_gnu(c: Codec, table: &mut [u8], hashes: &[u32]) -> Result<()> {
    let header = GnuHashHeader::read(c, table)?;
    let word = c.word_size();
    let bits = (word * 8) as u32;
    let start = header.symoffset as usize;
    if start > hashes.len() {
        return Err(malformed(".gnu.hash"));
    }
    let bloom_at = 16;
    let buckets_at = bloom_at + header.bloom_size as usize * word;
    let chains_at = buckets_at + header.nbuckets as usize * 4;
    let needed = chains_at + (hashes.len() - start) * 4;
    if table.len() < needed {
        return Err(malformed(".gnu.hash"));
    }

    let hashes = &hashes[start..];

    let mut bloom = vec![0u64; header.bloom_size as usize];
    for &h in hashes {
        let w = (h / bits) as usize % bloom.len();
        bloom[w] |= 1u64 << (h % bits);
        bloom[w] |= 1u64 << ((h >> header.bloom_shift) % bits);
    }
    for (i, &w) in bloom.iter().enumerate() {
        c.put_word(table, bloom_at + i * word, w);
    }

    let mut buckets = vec![0u32; header.nbuckets as usize];
    for (i, &h) in hashes.iter().enumerate().rev() {
        buckets[(h % header.nbuckets) as usize] = (start + i) as u32;
    }
    for (i, &b) in buckets.iter().enumerate() {
        c.put_u32(table, buckets_at + i * 4, b);
    }

    for (i, &h) in hashes.iter().enumerate() {
        let last = hashes
            .get(i + 1)
            .is_none_or(|&next| next % header.nbuckets != h % header.nbuckets);
        let value = (h & !1) | u32::from(last);
        c.put_u32(table, chains_at + i * 4, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_functions_match_known_values() {
        assert_eq!(sysv_hash(b""), 0);
        assert_eq!(sysv_hash(b"printf"), 0x077905a6);
        assert_eq!(sysv_hash(b"exit"), 0x0006cf04);
        assert_eq!(gnu_hash(b""), 0x0000_1505);
        assert_eq!(gnu_hash(b"printf"), 0x156b2bb8);
        assert_eq!(gnu_hash(b"exit"), 0x7c967e3f);
    }

    /// Look a symbol up the way the dynamic loader does.
    fn gnu_lookup(c: Codec, table: &[u8], names: &[&[u8]], name: &[u8]) -> Option<usize> {
        let header = GnuHashHeader::read(c, table).unwrap();
        let word = c.word_size();
        let bits = (word * 8) as u32;
        let h = gnu_hash(name);
        let bloom_word = c.word(table, 16 + ((h / bits) % header.bloom_size) as usize * word);
        let mask = (1u64 << (h % bits)) | (1u64 << ((h >> header.bloom_shift) % bits));
        if bloom_word & mask != mask {
            return None;
        }
        let buckets_at = 16 + header.bloom_size as usize * word;
        let chains_at = buckets_at + header.nbuckets as usize * 4;
        let mut i = c.u32(table, buckets_at + (h % header.nbuckets) as usize * 4) as usize;
        if i == 0 {
            return None;
        }
        loop {
            let ch = c.u32(table, chains_at + (i - header.symoffset as usize) * 4);
            if ch | 1 == h | 1 && names[i] == name {
                return Some(i);
            }
            if ch & 1 != 0 {
                return None;
            }
            i += 1;
        }
    }

    #[test]
    fn rebuilt_gnu_table_finds_every_symbol() {
        for is_64 in [true, false] {
            let c = Codec {
                is_64,
                little_endian: true,
            };
            let owned: Vec<Vec<u8>> = (0..200)
                .map(|i| format!("symbol_{i}").into_bytes())
                .collect();
            let mut names: Vec<&[u8]> = vec![b"", b"undefined_a", b"undefined_b"];
            names.extend(owned.iter().map(|n| n.as_slice()));

            let header = GnuHashHeader {
                nbuckets: 17,
                symoffset: 3,
                bloom_size: 4,
                bloom_shift: 6,
            };
            let hashes: Vec<u32> = names.iter().map(|n| gnu_hash(n)).collect();
            let order = gnu_symbol_order(header, &hashes);
            let names: Vec<&[u8]> = order.iter().map(|&i| names[i]).collect();

            let size = 16 + 4 * c.word_size() + 17 * 4 + (names.len() - 3) * 4;
            let mut table = vec![0u8; size];
            c.put_u32(&mut table, 0, 17);
            c.put_u32(&mut table, 4, 3);
            c.put_u32(&mut table, 8, 4);
            c.put_u32(&mut table, 12, 6);
            let hashes: Vec<u32> = names.iter().map(|n| gnu_hash(n)).collect();
            rebuild_gnu(c, &mut table, &hashes).unwrap();

            for (i, name) in names.iter().enumerate().skip(3) {
                assert_eq!(gnu_lookup(c, &table, &names, name), Some(i));
            }
            assert_eq!(gnu_lookup(c, &table, &names, b"undefined_a"), None);
            assert_eq!(gnu_lookup(c, &table, &names, b"not_there"), None);
        }
    }

    #[test]
    fn hashed_range_follows_the_chain_array() {
        let c = Codec {
            is_64: true,
            little_endian: false,
        };
        let header = GnuHashHeader {
            nbuckets: 1,
            symoffset: 1,
            bloom_size: 1,
            bloom_shift: 0,
        };
        // An empty chain array, as GNU ld writes when nothing is defined.
        assert_eq!(header.hashed_end(c, 16 + 8 + 4, 2).unwrap(), 1);
        assert_eq!(header.hashed_end(c, 16 + 8 + 4 + 4, 2).unwrap(), 2);
        assert_eq!(header.hashed_end(c, 16 + 8 + 4 + 40, 2).unwrap(), 2);
        assert!(header.hashed_end(c, 16, 2).is_err());
    }

    #[test]
    fn rebuilt_sysv_table_finds_every_symbol() {
        let c = Codec {
            is_64: true,
            little_endian: false,
        };
        let owned: Vec<Vec<u8>> = (0..50).map(|i| format!("s{i}").into_bytes()).collect();
        let mut names: Vec<&[u8]> = vec![b""];
        names.extend(owned.iter().map(|n| n.as_slice()));
        let nbucket = 7;
        let mut table = vec![0u8; (2 + nbucket + names.len()) * 4];
        c.put_u32(&mut table, 0, nbucket as u32);
        c.put_u32(&mut table, 4, names.len() as u32);
        let hashes: Vec<u32> = names.iter().map(|n| sysv_hash(n)).collect();
        rebuild_sysv(c, 4, &mut table, &hashes).unwrap();

        for (i, name) in names.iter().enumerate().skip(1) {
            let mut j = c.u32(&table, (2 + sysv_hash(name) as usize % nbucket) * 4) as usize;
            while j != 0 && names[j] != *name {
                j = c.u32(&table, (2 + nbucket + j) * 4) as usize;
            }
            assert_eq!(j, i);
        }
    }
}
