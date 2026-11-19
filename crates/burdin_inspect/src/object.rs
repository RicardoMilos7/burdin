//! Symbol sizes from a relocatable object file: ELF (Linux), Mach-O (macOS), and COFF (Windows), 64-bit
//! little-endian, the three formats the compiler's backend writes on its supported platforms.
//!
//! An ELF symbol records its own size. A COFF or Mach-O symbol records only where it starts, so its size is the
//! distance to the next symbol of the same section, and the last one runs to the end of the section. The reader
//! keeps the symbols that define something in a section and drops undefined ones, section markers, and debug entries.

/// A defined symbol and the bytes it occupies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub size: u64,
    /// Code, as opposed to data.
    pub function: bool,
}

/// The symbols of an object file, in the order its symbol table lists them.
pub fn read_symbols(bytes: &[u8]) -> Result<Vec<Symbol>, String> {
    if bytes.starts_with(b"\x7fELF") {
        return elf(bytes);
    }
    if bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) {
        return macho(bytes);
    }
    // COFF has no magic number: the first field is the machine type.
    if bytes.len() >= 20 && matches!(u16_at(bytes, 0), Some(0x8664 | 0xaa64)) {
        return coff(bytes);
    }
    Err("not an ELF, Mach-O, or COFF object file for x86-64 or arm64".to_string())
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn truncated<T>(value: Option<T>) -> Result<T, String> {
    value.ok_or_else(|| "the object file is truncated".to_string())
}

/// The NUL-terminated string at `at`.
fn c_str(b: &[u8], at: usize) -> Option<String> {
    let rest = b.get(at..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

fn elf(b: &[u8]) -> Result<Vec<Symbol>, String> {
    if b.get(4) != Some(&2) || b.get(5) != Some(&1) {
        return Err("only 64-bit little-endian ELF objects are supported".to_string());
    }
    let shoff = truncated(u64_at(b, 0x28))? as usize;
    let shentsize = truncated(u16_at(b, 0x3a))? as usize;
    let shnum = truncated(u16_at(b, 0x3c))? as usize;
    let section = |index: usize| shoff + index * shentsize;
    let mut out = Vec::new();
    for index in 0..shnum {
        let at = section(index);
        // SHT_SYMTAB
        if truncated(u32_at(b, at + 4))? != 2 {
            continue;
        }
        let offset = truncated(u64_at(b, at + 0x18))? as usize;
        let size = truncated(u64_at(b, at + 0x20))? as usize;
        let strtab = section(truncated(u32_at(b, at + 0x28))? as usize);
        let strings = truncated(u64_at(b, strtab + 0x18))? as usize;
        for entry in 0..size / 24 {
            let at = offset + entry * 24;
            let name = truncated(u32_at(b, at))? as usize;
            let info = truncated(b.get(at + 4).copied())?;
            let shndx = truncated(u16_at(b, at + 6))?;
            let size = truncated(u64_at(b, at + 16))?;
            // An undefined symbol (0) and the reserved indices (absolute, common) occupy nothing here.
            if shndx == 0 || shndx >= 0xff00 || size == 0 {
                continue;
            }
            // STT_OBJECT, STT_FUNC; section and file symbols are bookkeeping.
            let function = match info & 0xf {
                2 => true,
                1 | 0 => false,
                _ => continue,
            };
            let name = truncated(c_str(b, strings + name))?;
            if !name.is_empty() {
                out.push(Symbol { name, size, function });
            }
        }
    }
    Ok(out)
}

/// A symbol that knows where it starts and which section it is in; sizes come from the gaps.
struct Placed {
    name: String,
    section: usize,
    value: u64,
    function: bool,
}

/// Sizes the placed symbols by the distance to the next one in their section. `sections` gives each section's
/// extent as `(first address, length)`, indexed like `Placed::section`.
fn by_distance(mut placed: Vec<Placed>, sections: &[(u64, u64)]) -> Vec<Symbol> {
    let listed: Vec<(String, usize, u64, bool)> =
        placed.iter().map(|p| (p.name.clone(), p.section, p.value, p.function)).collect();
    placed.sort_by_key(|p| (p.section, p.value));
    let mut sizes: std::collections::HashMap<(usize, u64, String), u64> = std::collections::HashMap::new();
    for (i, p) in placed.iter().enumerate() {
        let (start, length) = sections.get(p.section).copied().unwrap_or((0, 0));
        let next = placed[i + 1..].iter().find(|q| q.section == p.section && q.value > p.value).map(|q| q.value);
        let end = next.unwrap_or(start + length);
        sizes.insert((p.section, p.value, p.name.clone()), end.saturating_sub(p.value));
    }
    listed
        .into_iter()
        .filter_map(|(name, section, value, function)| {
            let size = sizes[&(section, value, name.clone())];
            (size > 0).then_some(Symbol { name, size, function })
        })
        .collect()
}

fn macho(b: &[u8]) -> Result<Vec<Symbol>, String> {
    let commands = truncated(u32_at(b, 16))? as usize;
    let mut at = 32;
    // Section extents, numbered from 1 in the order the segments list them, and which of them hold code.
    let mut sections: Vec<(u64, u64)> = vec![(0, 0)];
    let mut code: Vec<bool> = vec![false];
    let mut symtab = None;
    for _ in 0..commands {
        let cmd = truncated(u32_at(b, at))?;
        let size = truncated(u32_at(b, at + 4))? as usize;
        match cmd {
            // LC_SEGMENT_64
            0x19 => {
                let count = truncated(u32_at(b, at + 64))? as usize;
                for n in 0..count {
                    let s = at + 72 + n * 80;
                    let name = truncated(b.get(s..s + 16))?;
                    let address = truncated(u64_at(b, s + 32))?;
                    let length = truncated(u64_at(b, s + 40))?;
                    sections.push((address, length));
                    code.push(name.starts_with(b"__text\0"));
                }
            }
            // LC_SYMTAB
            0x2 => {
                symtab = Some((
                    truncated(u32_at(b, at + 8))? as usize,
                    truncated(u32_at(b, at + 12))? as usize,
                    truncated(u32_at(b, at + 16))? as usize,
                ));
            }
            _ => {}
        }
        at += size.max(8);
    }
    let Some((offset, count, strings)) = symtab else { return Ok(Vec::new()) };
    let mut placed = Vec::new();
    for i in 0..count {
        let s = offset + i * 16;
        let name = truncated(u32_at(b, s))? as usize;
        let kind = truncated(b.get(s + 4).copied())?;
        let section = truncated(b.get(s + 5).copied())? as usize;
        let value = truncated(u64_at(b, s + 8))?;
        // Stabs (debug) are `n_type & 0xe0`; a symbol defined in a section has `N_SECT` in `n_type & 0x0e`.
        if kind & 0xe0 != 0 || kind & 0x0e != 0x0e || section == 0 || section >= sections.len() {
            continue;
        }
        let name = truncated(c_str(b, strings + name))?;
        // Every C-level symbol on Mach-O starts with an underscore.
        let name = name.strip_prefix('_').map(str::to_string).unwrap_or(name);
        placed.push(Placed { name, section, value, function: code[section] });
    }
    Ok(by_distance(placed, &sections))
}

fn coff(b: &[u8]) -> Result<Vec<Symbol>, String> {
    let count = truncated(u16_at(b, 2))? as usize;
    let table = truncated(u32_at(b, 8))? as usize;
    let symbols = truncated(u32_at(b, 12))? as usize;
    let optional = truncated(u16_at(b, 16))? as usize;
    // Section extents numbered from 1: the headers start after the file header and the optional header.
    let mut sections: Vec<(u64, u64)> = vec![(0, 0)];
    for n in 0..count {
        let at = 20 + optional + n * 40;
        sections.push((0, truncated(u32_at(b, at + 16))? as u64));
    }
    let strings = table + symbols * 18;
    let mut placed = Vec::new();
    let mut i = 0;
    while i < symbols {
        let at = table + i * 18;
        let aux = truncated(b.get(at + 17).copied())? as usize;
        let name = if truncated(u32_at(b, at))? == 0 {
            truncated(c_str(b, strings + truncated(u32_at(b, at + 4))? as usize))?
        } else {
            let raw = truncated(b.get(at..at + 8))?;
            let end = raw.iter().position(|&c| c == 0).unwrap_or(8);
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        let value = truncated(u32_at(b, at + 8))? as u64;
        let section = truncated(u16_at(b, at + 12))? as i16;
        let kind = truncated(u16_at(b, at + 14))?;
        let class = truncated(b.get(at + 16).copied())?;
        i += 1 + aux;
        // A section number of zero or below is undefined, absolute, or debug. A static symbol with auxiliary
        // records names a section, not a function or an object.
        if section <= 0 || section as usize >= sections.len() || (class == 3 && aux > 0) || name.starts_with('@') {
            continue;
        }
        placed.push(Placed { name, section: section as usize, value, function: kind >> 4 == 2 });
    }
    // A data label inside the text section's range is not a function: only a `DTYPE_FUNCTION` symbol is code.
    Ok(by_distance(placed, &sections))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: u16) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }
    fn le32(v: u32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }
    fn le64(v: u64) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    /// A minimal ELF64 object: a null section, a symbol table, and its string table.
    fn elf_object(symbols: &[(&str, u8, u16, u64)]) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut table = vec![0u8; 24];
        for (name, info, shndx, size) in symbols {
            let at = strings.len() as u32;
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
            table.extend(le32(at));
            table.push(*info);
            table.push(0);
            table.extend(le16(*shndx));
            table.extend(le64(0));
            table.extend(le64(*size));
        }
        let header_len = 64;
        let symtab_at = header_len;
        let strtab_at = symtab_at + table.len();
        let shoff = strtab_at + strings.len();
        let mut out = vec![0u8; header_len];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 2;
        out[5] = 1;
        out[0x28..0x30].copy_from_slice(&le64(shoff as u64));
        out[0x3a..0x3c].copy_from_slice(&le16(64));
        out[0x3c..0x3e].copy_from_slice(&le16(3));
        out.extend(&table);
        out.extend(&strings);
        let section = |kind: u32, offset: usize, size: usize, link: u32| {
            let mut s = vec![0u8; 64];
            s[4..8].copy_from_slice(&le32(kind));
            s[0x18..0x20].copy_from_slice(&le64(offset as u64));
            s[0x20..0x28].copy_from_slice(&le64(size as u64));
            s[0x28..0x2c].copy_from_slice(&le32(link));
            s
        };
        out.extend(section(0, 0, 0, 0));
        out.extend(section(2, symtab_at, table.len(), 2));
        out.extend(section(3, strtab_at, strings.len(), 0));
        out
    }

    #[test]
    fn elf_symbols_carry_their_own_size() {
        let object = elf_object(&[
            ("burdin.main", 0x12, 1, 90),
            ("burdin.vtable.0", 0x01, 2, 24),
            ("printf", 0x10, 0, 0),
            ("burdin.empty", 0x12, 1, 0),
            ("section", 0x03, 1, 10),
        ]);
        let symbols = read_symbols(&object).unwrap();
        assert_eq!(
            symbols,
            vec![
                Symbol { name: "burdin.main".into(), size: 90, function: true },
                Symbol { name: "burdin.vtable.0".into(), size: 24, function: false },
            ]
        );
    }

    /// A COFF object with one `.text` section of 100 bytes and the given `(name, offset, section, type)` symbols.
    fn coff_object(symbols: &[(&str, u32, i16, u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(le16(0x8664));
        out.extend(le16(1));
        out.extend(le32(0));
        let table = 20 + 40;
        out.extend(le32(table as u32));
        out.extend(le32(symbols.len() as u32));
        out.extend(le16(0));
        out.extend(le16(0));
        let mut header = vec![0u8; 40];
        header[..5].copy_from_slice(b".text");
        header[16..20].copy_from_slice(&le32(100));
        out.extend(header);
        let mut long = vec![0u8; 4];
        for (name, value, section, kind) in symbols {
            if name.len() <= 8 {
                let mut raw = [0u8; 8];
                raw[..name.len()].copy_from_slice(name.as_bytes());
                out.extend(raw);
            } else {
                out.extend(le32(0));
                out.extend(le32(long.len() as u32));
                long.extend_from_slice(name.as_bytes());
                long.push(0);
            }
            out.extend(le32(*value));
            out.extend(le16(*section as u16));
            out.extend(le16(*kind));
            out.push(2);
            out.push(0);
        }
        let total = long.len() as u32;
        long[..4].copy_from_slice(&le32(total));
        out.extend(long);
        out
    }

    #[test]
    fn coff_sizes_are_the_distance_to_the_next_symbol() {
        let object =
            coff_object(&[("burdin.first.function", 0, 1, 0x20), ("second", 40, 1, 0x20), ("undefined", 0, 0, 0x20)]);
        let symbols = read_symbols(&object).unwrap();
        assert_eq!(
            symbols,
            vec![
                Symbol { name: "burdin.first.function".into(), size: 40, function: true },
                Symbol { name: "second".into(), size: 60, function: true },
            ]
        );
    }

    /// A Mach-O object with a `__text` section of 64 bytes at address 0 and the given `(name, section, value)`.
    fn macho_object(symbols: &[(&str, u8, u64)]) -> Vec<u8> {
        let segment_size = 72 + 80;
        let symtab_size = 24;
        let mut out = Vec::new();
        out.extend([0xcf, 0xfa, 0xed, 0xfe]);
        out.extend(le32(0x0100_000c));
        out.extend(le32(0));
        out.extend(le32(1));
        out.extend(le32(2));
        out.extend(le32((segment_size + symtab_size) as u32));
        out.extend(le32(0));
        out.extend(le32(0));
        // LC_SEGMENT_64 with one section.
        out.extend(le32(0x19));
        out.extend(le32(segment_size as u32));
        out.extend(vec![0u8; 56]);
        out.extend(le32(1));
        out.extend(le32(0));
        let mut section = vec![0u8; 80];
        section[..6].copy_from_slice(b"__text");
        section[32..40].copy_from_slice(&le64(0));
        section[40..48].copy_from_slice(&le64(64));
        out.extend(section);
        let symoff = out.len() + symtab_size;
        let stroff = symoff + symbols.len() * 16;
        out.extend(le32(0x2));
        out.extend(le32(symtab_size as u32));
        out.extend(le32(symoff as u32));
        out.extend(le32(symbols.len() as u32));
        out.extend(le32(stroff as u32));
        out.extend(le32(0));
        let mut strings = vec![0u8];
        let mut table = Vec::new();
        for (name, section, value) in symbols {
            table.extend(le32(strings.len() as u32));
            table.push(if *section == 0 { 0x01 } else { 0x0f });
            table.push(*section);
            table.extend(le16(0));
            table.extend(le64(*value));
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
        }
        out.extend(table);
        out.extend(strings);
        out
    }

    #[test]
    fn mach_o_sizes_are_the_distance_to_the_next_symbol_and_names_lose_their_underscore() {
        let object = macho_object(&[("_burdin.a", 1, 0), ("_burdin.b", 1, 16), ("_malloc", 0, 0)]);
        let symbols = read_symbols(&object).unwrap();
        assert_eq!(
            symbols,
            vec![
                Symbol { name: "burdin.a".into(), size: 16, function: true },
                Symbol { name: "burdin.b".into(), size: 48, function: true },
            ]
        );
    }

    #[test]
    fn other_files_and_short_files_are_refused() {
        assert!(read_symbols(b"MZ").unwrap_err().contains("not an ELF"));
        let mut object = elf_object(&[("f", 0x12, 1, 4)]);
        object.truncate(100);
        assert!(read_symbols(&object).unwrap_err().contains("truncated"));
    }
}
