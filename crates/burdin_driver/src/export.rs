//! `burdin bindgen --export`: a C header for what a package exports (`spec/bindgen.md`, BG10 and BG11).
//!
//! The package is checked like any other compilation. Every `extern "C" fn` with a body that is not `priv`, in a
//! module of the entry package, becomes a prototype; the `repr(C)` structs its signature reaches (through pointers,
//! and through the fields of those structs) become definitions, and a struct that is not `repr(C)` becomes an opaque
//! declaration that is only used through pointers. The public integer, floating-point, string, and `bool` constants
//! of the modules that export a function become `#define`s. A signature C cannot spell is E0263 at the function.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use burdin_bindgen::export::{
    CType, ExportConstant, ExportFunction, ExportHeader, ExportStruct, guard_for, is_c_identifier,
};
use burdin_diag::{Code, Diagnostics, FileId};
use burdin_syntax::ast;
use burdin_tir::{ExprKind, FloatTy, IntTy, Program, Ty};

use crate::{CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, front_end, render_options};

pub(crate) fn run(target: &Path, out: &Path, guard: Option<&str>, diagnostics: &DiagnosticOptions) -> ExitCode {
    let options = CompileOptions {
        release: false,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        // A library package has no `main`, and the checks are the same.
        mode: EntryMode::Test,
        diagnostics,
        comptime_steps: None,
        emit_deps: None,
        run_build_scripts: true,
        locked: false,
        report_clean: false,
        jobs: crate::jobs_or_default(None),
    };
    let front = match front_end(target, &options, &SourceOverrides::new()) {
        Ok(front) => front,
        Err(code) => return code,
    };
    let render = render_options(diagnostics, std::io::stderr().is_terminal());
    if front.diags.has_errors() {
        eprint!("{}", front.diags.render_with(&front.sources, &render).text);
        eprintln!("error: could not check {} (fix the errors before exporting)", target.display());
        return ExitCode::from(1);
    }
    let Some(program) = front.program.as_ref() else {
        eprintln!("error: could not check {}", target.display());
        return ExitCode::from(1);
    };
    let stem = out.file_stem().and_then(|stem| stem.to_str()).unwrap_or("exports");
    let guard = guard.map_or_else(|| guard_for(stem), str::to_string);
    let mut diags = Diagnostics::default();
    let header = build_header(&front, program, guard, &mut diags);
    if diags.has_errors() {
        eprint!("{}", diags.render_with(&front.sources, &render).text);
        let total = diags.iter().count();
        let plural = if total == 1 { "" } else { "s" };
        eprintln!("error: could not write a C header for {} ({total} error{plural})", target.display());
        return ExitCode::from(1);
    }
    if header.functions.is_empty() {
        eprintln!("warning: {} exports no `extern \"C\"` function, so the header declares nothing", target.display());
    }
    match crate::bindgen::write_file(&out.display().to_string(), &header.render()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// The entry package's own modules, as file ids: the modules a build script generated under `target/` and the
/// embedded standard library are not the package's exports.
fn entry_files(front: &FrontEnd) -> Vec<FileId> {
    let mut files = Vec::new();
    let mut first = 0usize;
    for (index, package) in front.loaded.packages.iter().enumerate() {
        if index == front.loaded.entry_package {
            let generated = package.dir.as_ref().map(|dir| dir.join("target"));
            for (offset, module) in package.modules.iter().enumerate() {
                let outside = generated.as_ref().is_some_and(|dir| module.file.starts_with(dir));
                if module.text.is_none() && !outside {
                    files.push(FileId((first + offset) as u32));
                }
            }
        }
        first += package.modules.len();
    }
    files
}

fn build_header(front: &FrontEnd, program: &Program, guard: String, diags: &mut Diagnostics) -> ExportHeader {
    let mut header = ExportHeader { guard, ..ExportHeader::default() };
    let mut collector = Collector::new(program);
    let mut function_names: HashSet<String> = HashSet::new();
    let mut constant_files: Vec<FileId> = Vec::new();
    for file in entry_files(front) {
        let ast = &front.parsed[file.0 as usize].1;
        let mut exports_here = false;
        for item in &ast.items {
            let ast::Item::ExternFn(export) = item else { continue };
            let decl = &export.decl;
            if decl.is_priv {
                continue;
            }
            exports_here = true;
            let name = decl.name.name.clone();
            let span = decl.name.span;
            if !is_c_identifier(&name) {
                diags.error(Code::EXPORT_HEADER, span, format!("`{name}` is not a name a C header can declare"));
                continue;
            }
            if !function_names.insert(name.clone()) {
                diags.error(Code::EXPORT_HEADER, span, format!("`{name}` is exported twice under the same C name"));
                continue;
            }
            let Some(function) = program.functions.iter().find(|f| f.exported && f.name == name) else { continue };
            match collector.function(function) {
                Ok(function) => header.functions.push(function),
                Err(message) => {
                    diags.error(Code::EXPORT_HEADER, span, format!("cannot export `{name}` to a C header: {message}"));
                }
            }
        }
        if exports_here {
            constant_files.push(file);
        }
    }
    // The constants of the modules that export functions.
    let mut constant_names: HashSet<String> = HashSet::new();
    for file in constant_files {
        for item in &front.parsed[file.0 as usize].1.items {
            let ast::Item::Const(declaration) = item else { continue };
            if declaration.is_priv {
                continue;
            }
            let Some(constant) = program.consts.iter().find(|c| c.span == declaration.name.span) else { continue };
            let Some(value) = constant_text(&constant.value.kind, &constant.value.ty) else { continue };
            let name = declaration.name.name.clone();
            if !is_c_identifier(&name) {
                diags.error(
                    Code::EXPORT_HEADER,
                    declaration.name.span,
                    format!("the constant `{name}` is not a name a C header can declare"),
                );
                continue;
            }
            if function_names.contains(&name) || !constant_names.insert(name.clone()) {
                diags.error(
                    Code::EXPORT_HEADER,
                    declaration.name.span,
                    format!("`{name}` is exported twice under the same C name"),
                );
                continue;
            }
            header.constants.push(ExportConstant { name, value });
        }
    }
    header.structs = collector.structs;
    header
}

/// The text after `#define NAME` for a constant of a type C has a literal for, or `None` for any other value.
fn constant_text(kind: &ExprKind, ty: &Ty) -> Option<String> {
    match kind {
        ExprKind::Int(value) => {
            // A suffix keeps a 64-bit or unsigned value from being read as a narrower C type.
            let suffix = match ty {
                Ty::Int(IntTy::U32) => "U",
                Ty::Int(IntTy::I64 | IntTy::Isize) => "LL",
                Ty::Int(IntTy::U64 | IntTy::Usize) => "ULL",
                _ => "",
            };
            Some(if *value < 0 { format!("({value}{suffix})") } else { format!("{value}{suffix}") })
        }
        ExprKind::Float(value) => {
            let text = format!("{value:?}");
            let text = if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") };
            Some(if matches!(ty, Ty::Float(FloatTy::F32)) { format!("{text}f") } else { text })
        }
        ExprKind::Bool(value) => Some(if *value { "true" } else { "false" }.to_string()),
        ExprKind::Str(text) => {
            let mut out = String::from("\"");
            for byte in text.bytes() {
                match byte {
                    b'"' => out.push_str("\\\""),
                    b'\\' => out.push_str("\\\\"),
                    b'\n' => out.push_str("\\n"),
                    b'\r' => out.push_str("\\r"),
                    b'\t' => out.push_str("\\t"),
                    0x20..=0x7e => out.push(byte as char),
                    // Octal with three digits, so a following digit cannot extend the escape.
                    other => out.push_str(&format!("\\{other:03o}")),
                }
            }
            out.push('"');
            Some(out)
        }
        _ => None,
    }
}

/// Collects the structs the exported signatures reach while it converts types.
struct Collector<'a> {
    program: &'a Program,
    structs: Vec<ExportStruct>,
    /// The C name of each struct id seen, and which id owns each name.
    by_id: HashMap<usize, String>,
    by_name: HashMap<String, usize>,
}

impl<'a> Collector<'a> {
    fn new(program: &'a Program) -> Collector<'a> {
        Collector { program, structs: Vec::new(), by_id: HashMap::new(), by_name: HashMap::new() }
    }

    fn function(&mut self, function: &burdin_tir::Function) -> Result<ExportFunction, String> {
        let mut params = Vec::new();
        for id in &function.params {
            let local = &function.locals[id.0];
            let ty = self
                .convert(&local.ty, Position::Parameter)
                .map_err(|why| format!("parameter `{}`: {why}", local.name))?;
            let name = if is_c_identifier(&local.name) { local.name.clone() } else { format!("{}_", local.name) };
            params.push((name, ty));
        }
        let ret = match &function.ret {
            Ty::Unit => CType::Void,
            other => self.convert(other, Position::Result).map_err(|why| format!("the result: {why}"))?,
        };
        Ok(ExportFunction { name: function.name.clone(), ret, params })
    }

    fn convert(&mut self, ty: &Ty, position: Position) -> Result<CType, String> {
        match ty {
            Ty::Int(int) => Ok(CType::Scalar(match int {
                IntTy::I8 => "int8_t",
                IntTy::I16 => "int16_t",
                IntTy::I32 => "int32_t",
                IntTy::I64 => "int64_t",
                IntTy::Isize => "ptrdiff_t",
                IntTy::U8 => "uint8_t",
                IntTy::U16 => "uint16_t",
                IntTy::U32 => "uint32_t",
                IntTy::U64 => "uint64_t",
                IntTy::Usize => "size_t",
            })),
            Ty::Float(FloatTy::F32) => Ok(CType::Scalar("float")),
            Ty::Float(FloatTy::F64) => Ok(CType::Scalar("double")),
            Ty::Bool if position == Position::Field => Ok(CType::Scalar("bool")),
            Ty::CPtr(inner) => match inner.as_ref() {
                Ty::Unit => Ok(CType::Pointer(Box::new(CType::Void))),
                // `CChar` is `i8`: a pointer to it is C's `char *`, which is what C callers have.
                Ty::Int(IntTy::I8) => Ok(CType::Pointer(Box::new(CType::Scalar("char")))),
                Ty::Struct(adt) => {
                    let name = self.declare_struct(adt.id, &adt.name, false)?;
                    Ok(CType::Pointer(Box::new(CType::Struct(name))))
                }
                other => {
                    let pointee = self.convert(other, Position::Pointee)?;
                    Ok(CType::Pointer(Box::new(pointee)))
                }
            },
            Ty::Struct(adt) if position == Position::Field => {
                let name = self.declare_struct(adt.id, &adt.name, true)?;
                Ok(CType::Struct(name))
            }
            // As with pointers, an array of `i8` (`CChar`) is C's `char` array.
            Ty::Array(element, len) if position == Position::Field && **element == Ty::Int(IntTy::I8) => {
                Ok(CType::Array(Box::new(CType::Scalar("char")), *len))
            }
            Ty::Array(element, len) if position == Position::Field => {
                let element = self.convert(element, Position::Field)?;
                Ok(CType::Array(Box::new(element), *len))
            }
            other => Err(format!("the type `{other}` has no C spelling")),
        }
    }

    /// Declares a struct the first time it is reached. `by_value` says a field holds it directly, which needs its
    /// layout; a pointer to a struct that is not `repr(C)` only needs the name.
    fn declare_struct(&mut self, id: usize, display: &str, by_value: bool) -> Result<String, String> {
        if let Some(name) = self.by_id.get(&id) {
            return Ok(name.clone());
        }
        if display.contains('<') {
            return Err(format!("the generic struct `{display}` has no C declaration"));
        }
        let name = display.rsplit('.').next().unwrap_or(display).to_string();
        if !is_c_identifier(&name) {
            return Err(format!("the struct `{display}` is not a name a C header can declare"));
        }
        if let Some(owner) = self.by_name.get(&name)
            && *owner != id
        {
            return Err(format!("two structs are named `{name}`, which a C header cannot tell apart"));
        }
        self.by_id.insert(id, name.clone());
        self.by_name.insert(name.clone(), id);
        let definition = &self.program.structs[id];
        if !definition.repr_c {
            if by_value {
                return Err(format!(
                    "the struct `{name}` is held by value but is not `repr(C)`, so C cannot see its layout"
                ));
            }
            self.structs.push(ExportStruct { name: name.clone(), fields: Vec::new(), opaque: true });
            return Ok(name);
        }
        let mut fields = Vec::new();
        for field in &definition.fields {
            if !is_c_identifier(&field.name) {
                self.by_id.remove(&id);
                self.by_name.remove(&name);
                return Err(format!("the field `{}` of `{name}` is not a name a C header can declare", field.name));
            }
            let converted = self.convert(&field.ty, Position::Field).inspect_err(|_| {
                self.by_id.remove(&id);
                self.by_name.remove(&name);
            });
            let converted = converted.map_err(|why| format!("the field `{}` of `{name}`: {why}", field.name))?;
            fields.push((field.name.clone(), converted));
        }
        self.structs.push(ExportStruct { name: name.clone(), fields, opaque: false });
        Ok(name)
    }
}

/// Where a type appears, which decides what C can spell: a field may hold a struct, an array, or a `bool`; a
/// parameter or a result may not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Parameter,
    Result,
    Pointee,
    Field,
}
