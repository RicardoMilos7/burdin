//! `--emit=<kinds>` (`spec/inspect.md`, INS7-INS8): the artifacts of a build written next to the output, one file per
//! kind. This module parses the kinds and names the files; `compile` and `emit_program` in `main.rs` write them.

use burdin_codegen::AsmSyntax;
use std::path::{Path, PathBuf};

/// One artifact `--emit` can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmitKind {
    /// The typed tree of the program's own functions, as a debug dump (`<base>.tir`).
    Tir,
    /// The LLVM IR code generation produced, before optimization (`<base>.ll`).
    LlvmIr,
    /// The LLVM IR after the profile's optimization pipeline, what the backend compiles (`<base>.opt.ll`).
    LlvmIrOpt,
    /// The optimized module as LLVM bitcode (`<base>.bc`).
    LlvmBc,
    /// The program's assembly (`<base>.s`).
    Asm,
    /// The object file (`<base>.o`, or `<base>.obj` with MSVC).
    Obj,
    /// The files the build read besides the sources (`<base>.deps`), the same list as `--emit-deps`.
    Deps,
}

impl EmitKind {
    pub(crate) const ALL: [EmitKind; 7] = [
        EmitKind::Tir,
        EmitKind::LlvmIr,
        EmitKind::LlvmIrOpt,
        EmitKind::LlvmBc,
        EmitKind::Asm,
        EmitKind::Obj,
        EmitKind::Deps,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            EmitKind::Tir => "tir",
            EmitKind::LlvmIr => "llvm-ir",
            EmitKind::LlvmIrOpt => "llvm-ir-opt",
            EmitKind::LlvmBc => "llvm-bc",
            EmitKind::Asm => "asm",
            EmitKind::Obj => "obj",
            EmitKind::Deps => "deps",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            EmitKind::Tir => ".tir",
            EmitKind::LlvmIr => ".ll",
            EmitKind::LlvmIrOpt => ".opt.ll",
            EmitKind::LlvmBc => ".bc",
            EmitKind::Asm => ".s",
            EmitKind::Obj if cfg!(windows) => ".obj",
            EmitKind::Obj => ".o",
            EmitKind::Deps => ".deps",
        }
    }
}

/// What `--emit` asked for.
#[derive(Clone, Debug, Default)]
pub(crate) struct Emit {
    pub(crate) kinds: Vec<EmitKind>,
    pub(crate) asm_syntax: AsmSyntax,
    /// Stop after code generation: no executable is linked (`burdin asm` and `burdin bloat --no-link` read the
    /// artifacts only).
    pub(crate) skip_link: bool,
}

impl Emit {
    pub(crate) fn wants(&self, kind: EmitKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// The path of `kind`'s file for the build that writes `output`. `app` gives `app.ll`, `app.s`, ...; a platform
    /// executable extension (`app.exe`) is dropped first, so the artifacts share the executable's stem.
    pub(crate) fn path(&self, output: &Path, kind: EmitKind) -> PathBuf {
        artifact_path(output, kind)
    }
}

/// See [`Emit::path`].
pub(crate) fn artifact_path(output: &Path, kind: EmitKind) -> PathBuf {
    let exe = std::env::consts::EXE_EXTENSION;
    let base = if !exe.is_empty() && output.extension().is_some_and(|e| e == exe) {
        output.with_extension("")
    } else {
        output.to_path_buf()
    };
    let mut text = base.into_os_string();
    text.push(kind.suffix());
    PathBuf::from(text)
}

/// The path of the `index`-th object of a build split into several codegen units: `app.1.o`, `app.2.o`. Index 0 is
/// the plain `app.o`.
pub(crate) fn object_path(output: &Path, index: usize) -> PathBuf {
    let first = artifact_path(output, EmitKind::Obj);
    if index == 0 {
        return first;
    }
    let suffix = EmitKind::Obj.suffix();
    let text = first.to_string_lossy().into_owned();
    let stem = text.strip_suffix(suffix).unwrap_or(&text);
    PathBuf::from(format!("{stem}.{index}{suffix}"))
}

/// Parses the value of `--emit`: kind names separated by commas.
pub(crate) fn parse_kinds(value: &str) -> Result<Vec<EmitKind>, String> {
    let mut kinds = Vec::new();
    for word in value.split(',') {
        let word = word.trim();
        let Some(kind) = EmitKind::ALL.iter().copied().find(|k| k.name() == word) else {
            let names: Vec<&str> = EmitKind::ALL.iter().map(|k| k.name()).collect();
            return Err(format!("unknown `--emit` kind `{word}` (the kinds are {})", names.join(", ")));
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_as_a_comma_separated_list_without_repeats() {
        let kinds = parse_kinds("asm,llvm-ir,asm").unwrap();
        assert_eq!(kinds, vec![EmitKind::Asm, EmitKind::LlvmIr]);
        let err = parse_kinds("asm,ir").unwrap_err();
        assert!(err.contains("unknown `--emit` kind `ir`") && err.contains("llvm-ir-opt"), "{err}");
        assert!(parse_kinds("").is_err());
    }

    #[test]
    fn artifacts_sit_next_to_the_output_under_its_stem() {
        let out = Path::new("build/app");
        assert_eq!(artifact_path(out, EmitKind::Asm), PathBuf::from("build/app.s"));
        assert_eq!(artifact_path(out, EmitKind::LlvmIrOpt), PathBuf::from("build/app.opt.ll"));
        assert_eq!(artifact_path(out, EmitKind::Tir), PathBuf::from("build/app.tir"));
        let suffix = EmitKind::Obj.suffix();
        assert_eq!(object_path(out, 0), PathBuf::from(format!("build/app{suffix}")));
        assert_eq!(object_path(out, 2), PathBuf::from(format!("build/app.2{suffix}")));
    }

    #[test]
    fn the_executable_extension_is_not_part_of_the_stem() {
        let exe = std::env::consts::EXE_EXTENSION;
        if !exe.is_empty() {
            let out = PathBuf::from(format!("app.{exe}"));
            assert_eq!(artifact_path(&out, EmitKind::LlvmIr), PathBuf::from("app.ll"));
        }
        // A dotted name that is not the executable extension stays whole.
        assert_eq!(artifact_path(Path::new("my.app"), EmitKind::LlvmIr), PathBuf::from("my.app.ll"));
    }
}
