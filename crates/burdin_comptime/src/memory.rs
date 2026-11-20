//! The C heap a `burdin test --interpret` run models (`spec/test-runner.md`, "Interpreting a test", IN1-IN5): every
//! `__alloc`, `__realloc`, and `__dealloc`, and every pointer dereference through a `CPtr`, with the strict checks a
//! compiled program only gets from a sanitizer. A fault carries the source location of the operation that tripped it,
//! because the interpreter runs the typed tree, whose every expression keeps its span.
//!
//! A pointer is an integer: block 0 is null, and the address of a byte is `block << 32 | offset`. Integer
//! arithmetic and casts between `CPtr` and the integer types therefore work as they do in compiled code, and the
//! checks here decide what a dereference of the result means.

use super::interp::Fault;
use burdin_diag::Span;
use burdin_tir::{IntTy, Ty};

/// The address shift between a block and its bytes: a block is at most 4 GiB.
const OFFSET_BITS: u32 = 32;

/// The bytes of one allocation.
struct Block {
    bytes: Vec<u8>,
    /// One flag per byte: whether anything has been stored there yet (IN3).
    init: Vec<bool>,
    align: usize,
    freed: bool,
    /// Where the allocation was made, for the leak report (IN5).
    span: Span,
}

/// The modeled C heap.
#[derive(Default)]
pub(crate) struct Memory {
    blocks: Vec<Block>,
}

/// The block and byte offset an address names.
fn address(addr: i128) -> Option<(usize, usize)> {
    let block = usize::try_from(addr >> OFFSET_BITS).ok()?;
    let offset = usize::try_from(addr & ((1 << OFFSET_BITS) - 1)).ok()?;
    Some((block, offset))
}

fn encode(block: usize, offset: usize) -> i128 {
    ((block as i128) << OFFSET_BITS) | offset as i128
}

fn fault(what: String, span: Span) -> Fault {
    Fault::Panic(what, span)
}

/// How a type's bytes are read and written through a pointer, or `None` for a type the model does not cover.
pub(crate) fn scalar(ty: &Ty) -> Option<Scalar> {
    Some(match ty {
        Ty::Bool => Scalar::Bool,
        Ty::Char => Scalar::Char,
        Ty::Int(i) => Scalar::Int(*i),
        Ty::Float(burdin_tir::FloatTy::F32) => Scalar::F32,
        Ty::Float(burdin_tir::FloatTy::F64) => Scalar::F64,
        Ty::CPtr(_) => Scalar::Ptr,
        _ => return None,
    })
}

/// The kinds of value the heap stores (IN2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scalar {
    Bool,
    Char,
    Int(IntTy),
    F32,
    F64,
    Ptr,
}

impl Scalar {
    pub(crate) fn size(self) -> usize {
        match self {
            Scalar::Bool => 1,
            Scalar::Char => 4,
            Scalar::Int(i) => i.bits() as usize / 8,
            Scalar::F32 => 4,
            Scalar::F64 => 8,
            Scalar::Ptr => 8,
        }
    }

    pub(crate) fn align(self) -> usize {
        self.size()
    }
}

/// The bytes a scalar value is stored as.
pub(crate) fn encode_value(s: Scalar, n: i128) -> Vec<u8> {
    let size = s.size();
    match s {
        Scalar::Bool => vec![u8::from(n != 0)],
        Scalar::Char => (n as u32).to_le_bytes().to_vec(),
        Scalar::F32 => (n as u64 as u32).to_le_bytes().to_vec(),
        Scalar::F64 => (n as u64).to_le_bytes().to_vec(),
        Scalar::Int(_) | Scalar::Ptr => n.to_le_bytes()[..size].to_vec(),
    }
}

/// The integer a scalar's bytes hold, in the range of its type.
pub(crate) fn decode_value(s: Scalar, bytes: &[u8]) -> i128 {
    let mut buf = [0u8; 16];
    buf[..bytes.len()].copy_from_slice(bytes);
    let raw = i128::from_le_bytes(buf);
    match s {
        Scalar::Bool => i128::from(raw != 0),
        Scalar::Char => i128::from(raw as u32),
        Scalar::Int(i) => truncate(raw, i),
        Scalar::F32 => i128::from(f64::from(f32::from_bits(raw as u32)).to_bits()),
        Scalar::F64 => raw & ((1i128 << 64) - 1),
        Scalar::Ptr => raw,
    }
}

/// A value brought into its integer type's range, sign-extended from its byte width.
fn truncate(raw: i128, i: IntTy) -> i128 {
    let bits = i.bits();
    let mask = (1i128 << bits) - 1;
    let raw = if bits >= 128 { raw } else { raw & mask };
    if i.signed() && bits < 128 && (raw >> (bits - 1)) & 1 == 1 { raw - (1i128 << bits) } else { raw }
}

/// The float a `Float` value's bits hold, as the integer that stores it.
pub(crate) fn float_bits(ty: &Ty, x: f64) -> i128 {
    match ty {
        Ty::Float(burdin_tir::FloatTy::F32) => i128::from(u64::from((x as f32).to_bits())),
        _ => i128::from(x.to_bits()),
    }
}

impl Memory {
    /// `__alloc(size, align)`: a fresh block, aligned as asked, whose bytes are uninitialized (IN2).
    pub(crate) fn alloc(&mut self, size: usize, align: usize, span: Span) -> i128 {
        self.blocks.push(Block {
            bytes: vec![0; size],
            init: vec![false; size],
            align: align.max(1),
            freed: false,
            span,
        });
        encode(self.blocks.len(), 0)
    }

    /// `__realloc(ptr, old_size, new_size, align)`: the contents grow or shrink in place; a freed pointer is a use
    /// after free (IN4), and a new, larger block's tail is uninitialized.
    pub(crate) fn realloc(&mut self, addr: i128, new_size: usize, span: Span) -> Result<i128, Fault> {
        let (index, offset) = self.locate(addr, 0, span)?;
        if offset != 0 {
            return Err(fault("reallocating a pointer that is not the start of its block".to_string(), span));
        }
        let block = &mut self.blocks[index - 1];
        block.bytes.resize(new_size, 0);
        block.init.resize(new_size, false);
        Ok(addr)
    }

    /// `__dealloc(ptr)`: the block is free from now on; freeing it again is a double free (IN4).
    pub(crate) fn release(&mut self, addr: i128, span: Span) -> Result<(), Fault> {
        let Some((index, offset)) = address(addr) else {
            return Err(fault("freeing a null pointer".to_string(), span));
        };
        let Some(block) = index.checked_sub(1).and_then(|i| self.blocks.get_mut(i)) else {
            return Err(fault("freeing a null pointer".to_string(), span));
        };
        if block.freed {
            return Err(fault(format!("double free: block {index} was already freed"), span));
        }
        if offset != 0 {
            return Err(fault("freeing a pointer that is not the start of its block".to_string(), span));
        }
        block.freed = true;
        Ok(())
    }

    /// The block an address names, checked for null, for a freed block, and for a range inside it.
    fn locate(&self, addr: i128, len: usize, span: Span) -> Result<(usize, usize), Fault> {
        let Some((index, offset)) = address(addr) else {
            return Err(fault("null pointer dereference".to_string(), span));
        };
        let Some(block) = index.checked_sub(1).and_then(|i| self.blocks.get(i)) else {
            return Err(fault("null pointer dereference".to_string(), span));
        };
        if block.freed {
            return Err(fault(format!("use after free: block {index} was freed"), span));
        }
        if offset.checked_add(len).is_none_or(|end| end > block.bytes.len()) {
            return Err(fault(
                format!(
                    "out-of-bounds access: {len} byte(s) at offset {offset} of a {}-byte allocation",
                    block.bytes.len()
                ),
                span,
            ));
        }
        Ok((index, offset))
    }

    /// Reads `s` at `addr`, checking null, the bounds, the alignment, the freed flag, and that every byte was
    /// written (IN3-IN4).
    pub(crate) fn read(&self, addr: i128, s: Scalar, span: Span) -> Result<i128, Fault> {
        let (index, offset) = self.locate(addr, s.size(), span)?;
        let block = &self.blocks[index - 1];
        if !offset.is_multiple_of(s.align()) || !block.align.is_multiple_of(s.align()) {
            return Err(fault(
                format!(
                    "misaligned access: offset {offset} of a {}-byte-aligned block for a {}-byte type",
                    block.align,
                    s.align()
                ),
                span,
            ));
        }
        if block.init[offset..offset + s.size()].iter().any(|set| !set) {
            return Err(fault("read of uninitialized memory".to_string(), span));
        }
        Ok(decode_value(s, &block.bytes[offset..offset + s.size()]))
    }

    /// Writes `n` as `s` at `addr`, with the same checks as a read.
    pub(crate) fn write(&mut self, addr: i128, s: Scalar, n: i128, span: Span) -> Result<(), Fault> {
        let (index, offset) = self.locate(addr, s.size(), span)?;
        let block = &mut self.blocks[index - 1];
        if !offset.is_multiple_of(s.align()) || !block.align.is_multiple_of(s.align()) {
            return Err(fault(
                format!(
                    "misaligned access: offset {offset} of a {}-byte-aligned block for a {}-byte type",
                    block.align,
                    s.align()
                ),
                span,
            ));
        }
        let bytes = encode_value(s, n);
        block.bytes[offset..offset + bytes.len()].copy_from_slice(&bytes);
        for set in &mut block.init[offset..offset + bytes.len()] {
            *set = true;
        }
        Ok(())
    }

    /// The allocations no `__dealloc` freed, with where each was made (IN5).
    pub(crate) fn leaks(&self) -> Vec<(Span, usize)> {
        self.blocks.iter().filter(|b| !b.freed).map(|b| (b.span, b.bytes.len())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::FileId;

    fn span() -> Span {
        Span::new(FileId(0), 0, 1)
    }

    #[test]
    fn a_fresh_block_reads_as_uninitialized() {
        let mut mem = Memory::default();
        let p = mem.alloc(8, 8, span());
        assert!(
            matches!(mem.read(p, Scalar::Int(IntTy::I32), span()), Err(Fault::Panic(m, _)) if m.contains("uninitialized"))
        );
        mem.write(p, Scalar::Int(IntTy::I32), -3, span()).unwrap();
        assert_eq!(mem.read(p, Scalar::Int(IntTy::I32), span()).unwrap(), -3);
    }

    #[test]
    fn bounds_alignment_and_the_freed_flag_are_checked() {
        let mut mem = Memory::default();
        let p = mem.alloc(8, 8, span());
        mem.write(p + 1, Scalar::Int(IntTy::I64), 1, span()).unwrap_err();
        mem.write(p + 8, Scalar::Int(IntTy::I32), 1, span()).unwrap_err();
        assert!(mem.read(p + 4, Scalar::Int(IntTy::I64), span()).is_err());
        mem.release(p, span()).unwrap();
        assert!(mem.read(p, Scalar::Int(IntTy::I32), span()).is_err());
        assert!(mem.release(p, span()).is_err());
        assert!(mem.leaks().is_empty());
    }

    #[test]
    fn an_unfreed_block_is_a_leak_with_its_span() {
        let mut mem = Memory::default();
        mem.alloc(16, 8, span());
        assert_eq!(mem.leaks().len(), 1);
    }

    #[test]
    fn a_pointer_round_trips_through_its_address() {
        let mut mem = Memory::default();
        let p = mem.alloc(4, 4, span());
        let (block, offset) = address(p).unwrap();
        assert_eq!(encode(block, offset), p);
        assert_eq!(address(0), Some((0, 0)));
    }

    #[test]
    fn integer_bytes_are_truncated_to_their_type() {
        assert_eq!(decode_value(Scalar::Int(IntTy::I8), &[0xff]), -1);
        assert_eq!(decode_value(Scalar::Int(IntTy::U8), &[0xff]), 255);
        assert_eq!(decode_value(Scalar::Bool, &[2]), 1);
    }
}
