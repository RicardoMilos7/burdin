//! Inspecting what the compiler generated (`spec/inspect.md`): the monomorphization report, the binary-size report
//! behind `burdin bloat`, the per-function assembly view behind `burdin asm`, and the source expansion behind
//! `burdin expand`. Everything here reads what the compiler built and changes nothing; it never links LLVM, and the
//! driver is the only caller.

pub mod asm;
pub mod bloat;
pub mod expand;
pub mod instances;
pub mod object;
pub mod reach;
pub mod tir;
