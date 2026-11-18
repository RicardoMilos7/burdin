//! libclang, loaded at run time.
//!
//! The compiler does not link libclang: `burdin bindgen` opens the shared library when it runs and looks up the
//! few dozen functions of the stable C API (`clang-c/Index.h`) it needs. A machine without libclang can still build
//! and use every other part of the compiler, and the tool says where it looked when it cannot find the library
//! (`spec/bindgen.md`, BG1). The structs below mirror the C structs, which have not changed since libclang 3.x.
//!
//! The wrappers are small and own nothing the translation unit does not own: a [`Cursor`] or a [`Ty`] borrows the
//! [`TranslationUnit`] it came from.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_longlong, c_uint, c_ulong, c_ulonglong, c_void};
use std::path::{Path, PathBuf};
use std::rc::Rc;

// --- The C structs ----------------------------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct CxString {
    data: *const c_void,
    private_flags: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CxCursor {
    kind: c_int,
    xdata: c_int,
    data: [*const c_void; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CxType {
    kind: c_int,
    data: [*mut c_void; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CxSourceLocation {
    ptr_data: [*const c_void; 2],
    int_data: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CxSourceRange {
    ptr_data: [*const c_void; 2],
    begin_int_data: c_uint,
    end_int_data: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CxToken {
    int_data: [c_uint; 4],
    ptr_data: *mut c_void,
}

#[repr(C)]
struct CxUnsavedFile {
    filename: *const c_char,
    contents: *const c_char,
    length: c_ulong,
}

type Visitor = extern "C" fn(CxCursor, CxCursor, *mut c_void) -> c_int;

// --- The function table -----------------------------------------------------------------------------------------

macro_rules! api {
    ($( fn $name:ident($($ty:ty),*) $(-> $ret:ty)?; )*) => {
        #[allow(non_snake_case)]
        struct Api {
            $( $name: unsafe extern "C" fn($($ty),*) $(-> $ret)?, )*
        }

        impl Api {
            fn load(library: &Library) -> Result<Api, String> {
                Ok(Api {
                    $( $name: library
                        .symbol(stringify!($name))
                        .ok_or_else(|| format!("the library has no `{}`", stringify!($name)))?, )*
                })
            }
        }
    };
}

api! {
    fn clang_createIndex(c_int, c_int) -> *mut c_void;
    fn clang_disposeIndex(*mut c_void);
    fn clang_parseTranslationUnit2(
        *mut c_void, *const c_char, *const *const c_char, c_int, *mut CxUnsavedFile, c_uint, c_uint,
        *mut *mut c_void
    ) -> c_int;
    fn clang_disposeTranslationUnit(*mut c_void);
    fn clang_getNumDiagnostics(*mut c_void) -> c_uint;
    fn clang_getDiagnostic(*mut c_void, c_uint) -> *mut c_void;
    fn clang_getDiagnosticSeverity(*mut c_void) -> c_int;
    fn clang_formatDiagnostic(*mut c_void, c_uint) -> CxString;
    fn clang_defaultDiagnosticDisplayOptions() -> c_uint;
    fn clang_disposeDiagnostic(*mut c_void);
    fn clang_getTranslationUnitCursor(*mut c_void) -> CxCursor;
    fn clang_visitChildren(CxCursor, Visitor, *mut c_void) -> c_uint;
    fn clang_getCursorKind(CxCursor) -> c_int;
    fn clang_getCursorSpelling(CxCursor) -> CxString;
    fn clang_getCString(CxString) -> *const c_char;
    fn clang_disposeString(CxString);
    fn clang_getCursorType(CxCursor) -> CxType;
    fn clang_getCursorResultType(CxCursor) -> CxType;
    fn clang_getTypedefDeclUnderlyingType(CxCursor) -> CxType;
    fn clang_getEnumDeclIntegerType(CxCursor) -> CxType;
    fn clang_getEnumConstantDeclValue(CxCursor) -> c_longlong;
    fn clang_getEnumConstantDeclUnsignedValue(CxCursor) -> c_ulonglong;
    fn clang_Cursor_getNumArguments(CxCursor) -> c_int;
    fn clang_Cursor_getArgument(CxCursor, c_uint) -> CxCursor;
    fn clang_Cursor_isVariadic(CxCursor) -> c_uint;
    fn clang_isFunctionTypeVariadic(CxType) -> c_uint;
    fn clang_getNumArgTypes(CxType) -> c_int;
    fn clang_getArgType(CxType, c_uint) -> CxType;
    fn clang_getResultType(CxType) -> CxType;
    fn clang_getPointeeType(CxType) -> CxType;
    fn clang_getCanonicalType(CxType) -> CxType;
    fn clang_getArrayElementType(CxType) -> CxType;
    fn clang_getArraySize(CxType) -> c_longlong;
    fn clang_Type_getNamedType(CxType) -> CxType;
    fn clang_getTypeDeclaration(CxType) -> CxCursor;
    fn clang_getTypeSpelling(CxType) -> CxString;
    fn clang_Type_getSizeOf(CxType) -> c_longlong;
    fn clang_Type_getAlignOf(CxType) -> c_longlong;
    fn clang_Cursor_getOffsetOfField(CxCursor) -> c_longlong;
    fn clang_Cursor_isBitField(CxCursor) -> c_uint;
    fn clang_isCursorDefinition(CxCursor) -> c_uint;
    fn clang_getCursorDefinition(CxCursor) -> CxCursor;
    fn clang_Cursor_getStorageClass(CxCursor) -> c_int;
    fn clang_Cursor_isFunctionInlined(CxCursor) -> c_uint;
    fn clang_Cursor_isMacroFunctionLike(CxCursor) -> c_uint;
    fn clang_Cursor_isMacroBuiltin(CxCursor) -> c_uint;
    fn clang_getCursorLocation(CxCursor) -> CxSourceLocation;
    fn clang_Location_isFromMainFile(CxSourceLocation) -> c_int;
    fn clang_getFileLocation(CxSourceLocation, *mut *mut c_void, *mut c_uint, *mut c_uint, *mut c_uint);
    fn clang_getFileName(*mut c_void) -> CxString;
    fn clang_getCursorExtent(CxCursor) -> CxSourceRange;
    fn clang_tokenize(*mut c_void, CxSourceRange, *mut *mut CxToken, *mut c_uint);
    fn clang_getTokenKind(CxToken) -> c_int;
    fn clang_getTokenSpelling(*mut c_void, CxToken) -> CxString;
    fn clang_disposeTokens(*mut c_void, *mut CxToken, c_uint);
    fn clang_Cursor_Evaluate(CxCursor) -> *mut c_void;
    fn clang_EvalResult_getKind(*mut c_void) -> c_int;
    fn clang_EvalResult_getAsLongLong(*mut c_void) -> c_longlong;
    fn clang_EvalResult_isUnsignedInt(*mut c_void) -> c_uint;
    fn clang_EvalResult_getAsUnsigned(*mut c_void) -> c_ulonglong;
    fn clang_EvalResult_getAsDouble(*mut c_void) -> c_double;
    fn clang_EvalResult_dispose(*mut c_void);
    fn clang_getClangVersion() -> CxString;
}

// --- Loading the shared library ---------------------------------------------------------------------------------

#[cfg(unix)]
struct Library(*mut c_void);

#[cfg(unix)]
impl Library {
    fn open(path: &str) -> Option<Library> {
        let name = CString::new(path).ok()?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() { None } else { Some(Library(handle)) }
    }

    fn symbol<T: Copy>(&self, name: &str) -> Option<T> {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        let name = CString::new(name).ok()?;
        // SAFETY: the handle came from `dlopen`, and `name` is a NUL-terminated string.
        let address = unsafe { libc::dlsym(self.0, name.as_ptr()) };
        // SAFETY: `T` is a function pointer type with the size of a data pointer.
        if address.is_null() { None } else { Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) }) }
    }
}

#[cfg(windows)]
struct Library(*mut c_void);

#[cfg(windows)]
unsafe extern "system" {
    fn LoadLibraryA(name: *const c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

#[cfg(windows)]
impl Library {
    fn open(path: &str) -> Option<Library> {
        let name = CString::new(path).ok()?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        let handle = unsafe { LoadLibraryA(name.as_ptr()) };
        if handle.is_null() { None } else { Some(Library(handle)) }
    }

    fn symbol<T: Copy>(&self, name: &str) -> Option<T> {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        let name = CString::new(name).ok()?;
        // SAFETY: the handle came from `LoadLibraryA`, and `name` is a NUL-terminated string.
        let address = unsafe { GetProcAddress(self.0, name.as_ptr()) };
        // SAFETY: `T` is a function pointer type with the size of a data pointer.
        if address.is_null() { None } else { Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) }) }
    }
}

/// Where to look for libclang, most specific first. An explicit `BURDIN_LIBCLANG` is the only candidate when it is
/// set, so a wrong path is reported instead of silently replaced by another library.
fn candidates() -> Vec<String> {
    if let Some(path) = std::env::var_os("BURDIN_LIBCLANG").filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path).display().to_string()];
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("LIBCLANG_PATH").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return vec![path.display().to_string()];
        }
        dirs.push(path);
    }
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX").filter(|prefix| !prefix.is_empty()) {
        let prefix = PathBuf::from(prefix);
        dirs.push(prefix.join("lib"));
        dirs.push(prefix.join("bin"));
    }
    if let Some(dir) = llvm_config_libdir() {
        dirs.push(dir);
    }
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\Program Files\LLVM\bin"));
    } else {
        // Debian and Ubuntu keep one tree per LLVM version; the pinned version goes first.
        let mut trees: Vec<PathBuf> = std::fs::read_dir("/usr/lib")
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|n| n.starts_with("llvm-")))
            .collect();
        trees.sort_by_key(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_string();
            (name != "llvm-21", std::cmp::Reverse(name))
        });
        dirs.extend(trees.into_iter().map(|tree| tree.join("lib")));
        for dir in [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
            "/usr/local/lib",
            "/opt/homebrew/opt/llvm@21/lib",
            "/opt/homebrew/opt/llvm/lib",
            "/usr/local/opt/llvm@21/lib",
            "/usr/local/opt/llvm/lib",
            "/Library/Developer/CommandLineTools/usr/lib",
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
        ] {
            dirs.push(PathBuf::from(dir));
        }
    }
    let mut found: Vec<String> = Vec::new();
    for dir in dirs {
        found.extend(libraries_in(&dir));
    }
    // Finally the dynamic loader's own search path.
    if cfg!(windows) {
        found.push("libclang.dll".to_string());
    } else if cfg!(target_os = "macos") {
        found.push("libclang.dylib".to_string());
    } else {
        found.extend(["libclang-21.so.1", "libclang.so.1", "libclang.so"].map(str::to_string));
    }
    found
}

/// The libclang files in `dir`: `libclang.so`, `libclang-21.so.1`, `libclang.dylib`, `libclang.dll`. `libclang-cpp`
/// is the C++ API and is not it.
fn libraries_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| {
            name.starts_with("libclang") && !name.starts_with("libclang-cpp") && !name.starts_with("libclang_rt")
        })
        .filter(|name| name.contains(".so") || name.ends_with(".dylib") || name.ends_with(".dll"))
        .collect();
    names.sort_by_key(|name| (!name.contains("21"), name.clone()));
    names.into_iter().map(|name| dir.join(name).display().to_string()).collect()
}

fn llvm_config_libdir() -> Option<PathBuf> {
    let program = match std::env::var_os("LLVM_SYS_211_PREFIX").filter(|prefix| !prefix.is_empty()) {
        Some(prefix) => PathBuf::from(prefix).join("bin").join("llvm-config"),
        None => PathBuf::from("llvm-config"),
    };
    let output = std::process::Command::new(program).arg("--libdir").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.lines().next()?.trim();
    if line.is_empty() { None } else { Some(PathBuf::from(line)) }
}

/// Where clang's built-in headers are, given the library that was loaded and its version. Every candidate must
/// contain `include/stddef.h`, so a directory of some other compiler is never passed as this one's.
fn resource_dir_for(library: &str, version: &str) -> Option<String> {
    let major = version_major(version)?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    let library = Path::new(library);
    if let Some(dir) = library.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        // An LLVM tree keeps them under `lib/clang/<major>` next to the library; the Windows archive keeps the
        // library in `bin`, so `lib` is one level up.
        candidates.push(dir.join("clang").join(&major));
        candidates.push(dir.join("..").join("lib").join("clang").join(&major));
    }
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX").filter(|prefix| !prefix.is_empty()) {
        candidates.push(PathBuf::from(prefix).join("lib").join("clang").join(&major));
    }
    candidates.push(PathBuf::from(format!("/usr/lib/llvm-{major}/lib/clang/{major}")));
    for formula in [format!("/opt/homebrew/opt/llvm@{major}"), format!("/usr/local/opt/llvm@{major}")] {
        candidates.push(PathBuf::from(formula).join("lib").join("clang").join(&major));
    }
    candidates
        .into_iter()
        .find(|dir| dir.join("include").join("stddef.h").is_file())
        .map(|dir| dir.display().to_string())
}

/// The major version of `clang version 21.1.8`, from the version text.
pub(crate) fn version_major(version: &str) -> Option<String> {
    version.split_whitespace().find_map(|word| {
        let mut parts = word.split('.');
        let major = parts.next()?;
        parts.next()?;
        if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Some(major.to_string())
    })
}

// --- The safe layer ---------------------------------------------------------------------------------------------

/// The loaded library. Cheap to clone; the library stays loaded for the life of the process.
#[derive(Clone)]
pub struct Clang {
    api: Rc<Api>,
    version: String,
    resource_dir: Option<String>,
}

impl Clang {
    /// Finds and loads libclang. The error names every place that was tried.
    pub fn load() -> Result<Clang, String> {
        let tried = candidates();
        let mut failures: Vec<String> = Vec::new();
        for path in &tried {
            let Some(library) = Library::open(path) else { continue };
            match Api::load(&library) {
                Ok(api) => {
                    // `Library` has no `Drop`: letting it go out of scope leaves the handle open for the life of
                    // the process, which is what the function pointers in `api` need.
                    let api = Rc::new(api);
                    // SAFETY: the table was just loaded from the library.
                    let version = unsafe { take_string(&api, (api.clang_getClangVersion)()) };
                    let resource_dir = resource_dir_for(path, &version);
                    return Ok(Clang { api, version, resource_dir });
                }
                Err(message) => failures.push(format!("{path}: {message}")),
            }
        }
        let mut message = String::from("cannot load libclang");
        if !failures.is_empty() {
            message.push_str(&format!(" (found a library that is not usable: {})", failures.join("; ")));
        }
        message.push_str(
            "\nnote: install LLVM's libclang (`libclang-21-dev` on Debian and Ubuntu, `brew install llvm@21` on macOS, \
             the LLVM release archive on Windows) or set BURDIN_LIBCLANG to the library's path",
        );
        let shown: Vec<&String> = tried.iter().take(6).collect();
        message.push_str(&format!(
            "\nnote: looked at {}{}",
            shown.iter().map(|path| path.as_str()).collect::<Vec<_>>().join(", "),
            if tried.len() > shown.len() { ", ..." } else { "" }
        ));
        Err(message)
    }

    /// The text `clang_getClangVersion` reports, such as `Ubuntu clang version 21.1.8`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The directory of clang's built-in headers (`stddef.h`, `stdint.h`, ...), when it was found. The command
    /// passes it as `-resource-dir`: libclang derives that directory from where its shared library sits, which is
    /// wrong wherever a distribution keeps the library apart from the rest of the LLVM tree, as Ubuntu does with the
    /// multiarch directory, and every `<stddef.h>` then goes missing (BG1, BG2).
    pub fn resource_dir(&self) -> Option<&str> {
        self.resource_dir.as_deref()
    }

    /// Parses `file` as a translation unit with `args` as the compiler's command line. The preprocessing record is
    /// kept so macro definitions are visible. Function bodies are kept too: `CXTranslationUnit_SkipFunctionBodies`
    /// would make every function look like a mere declaration, and whether a `static` function has a body is exactly
    /// what decides between a wrapper and a skip (BG8).
    pub fn parse(&self, file: &Path, args: &[String]) -> Result<TranslationUnit, String> {
        const DETAILED_PREPROCESSING_RECORD: c_uint = 0x01;
        let api = &self.api;
        // SAFETY: every pointer passed below points into values that live until the call returns, and the index and
        // the unit are disposed by `TranslationUnit::drop`.
        unsafe {
            let index = (api.clang_createIndex)(0, 0);
            let file = CString::new(file.display().to_string()).map_err(|_| "the header path has a NUL".to_string())?;
            let owned: Vec<CString> = args
                .iter()
                .map(|arg| CString::new(arg.as_str()).map_err(|_| format!("the argument `{arg}` has a NUL")))
                .collect::<Result<_, _>>()?;
            let pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
            let mut unit: *mut c_void = std::ptr::null_mut();
            let status = (api.clang_parseTranslationUnit2)(
                index,
                file.as_ptr(),
                pointers.as_ptr(),
                pointers.len() as c_int,
                std::ptr::null_mut(),
                0,
                DETAILED_PREPROCESSING_RECORD,
                &mut unit,
            );
            if status != 0 || unit.is_null() {
                (api.clang_disposeIndex)(index);
                return Err(format!("libclang could not parse the header (status {status})"));
            }
            Ok(TranslationUnit { api: self.api.clone(), index, unit })
        }
    }
}

/// One diagnostic that libclang reported while parsing.
pub struct Diagnostic {
    pub is_error: bool,
    pub text: String,
}

/// A parsed header and everything that came from it. Cursors and types borrow it.
pub struct TranslationUnit {
    api: Rc<Api>,
    index: *mut c_void,
    unit: *mut c_void,
}

impl Drop for TranslationUnit {
    fn drop(&mut self) {
        // SAFETY: both handles were created by `Clang::parse` and are disposed exactly once.
        unsafe {
            (self.api.clang_disposeTranslationUnit)(self.unit);
            (self.api.clang_disposeIndex)(self.index);
        }
    }
}

/// Copies a libclang string and releases it.
unsafe fn take_string(api: &Api, string: CxString) -> String {
    // SAFETY: the caller passes a string libclang just returned, which is released here and not used again.
    unsafe {
        let pointer = (api.clang_getCString)(string);
        let text =
            if pointer.is_null() { String::new() } else { CStr::from_ptr(pointer).to_string_lossy().into_owned() };
        (api.clang_disposeString)(string);
        text
    }
}

impl TranslationUnit {
    /// Every diagnostic, errors and warnings alike, formatted the way clang prints them.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let api = &self.api;
        // SAFETY: the unit is alive, and each diagnostic is disposed after it is read.
        unsafe {
            let count = (api.clang_getNumDiagnostics)(self.unit);
            let mut found = Vec::new();
            for index in 0..count {
                let diagnostic = (api.clang_getDiagnostic)(self.unit, index);
                let severity = (api.clang_getDiagnosticSeverity)(diagnostic);
                let options = (api.clang_defaultDiagnosticDisplayOptions)();
                let text = take_string(api, (api.clang_formatDiagnostic)(diagnostic, options));
                (api.clang_disposeDiagnostic)(diagnostic);
                found.push(Diagnostic { is_error: severity >= 3, text });
            }
            found
        }
    }

    /// The translation unit's own cursor: the parent of every top-level declaration.
    pub fn root(&self) -> Cursor<'_> {
        // SAFETY: the unit is alive.
        let raw = unsafe { (self.api.clang_getTranslationUnitCursor)(self.unit) };
        Cursor { unit: self, raw }
    }
}

extern "C" fn collect_child(cursor: CxCursor, _parent: CxCursor, data: *mut c_void) -> c_int {
    // SAFETY: `data` is the `Vec<CxCursor>` that `Cursor::children` passed to `clang_visitChildren`.
    let children = unsafe { &mut *(data as *mut Vec<CxCursor>) };
    children.push(cursor);
    1 // CXChildVisit_Continue
}

/// The kinds of cursor the generator looks at (`CXCursorKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Struct,
    Union,
    Enum,
    Field,
    EnumConstant,
    Function,
    Variable,
    Parameter,
    Typedef,
    MacroDefinition,
    Other(i32),
}

impl CursorKind {
    fn from_raw(raw: c_int) -> CursorKind {
        match raw {
            2 => CursorKind::Struct,
            3 => CursorKind::Union,
            5 => CursorKind::Enum,
            6 => CursorKind::Field,
            7 => CursorKind::EnumConstant,
            8 => CursorKind::Function,
            9 => CursorKind::Variable,
            10 => CursorKind::Parameter,
            20 => CursorKind::Typedef,
            501 => CursorKind::MacroDefinition,
            other => CursorKind::Other(other),
        }
    }
}

/// How a function was declared (`CX_StorageClass`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    None,
    Extern,
    Static,
    Other,
}

/// A place in the source: file name, line, and whether it is in the file the tool was asked to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub offset: u32,
    pub in_main_file: bool,
}

/// The result of evaluating a constant expression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Evaluated {
    Int(i128),
    Float(f64),
}

#[derive(Clone, Copy)]
pub struct Cursor<'a> {
    unit: &'a TranslationUnit,
    raw: CxCursor,
}

impl<'a> Cursor<'a> {
    fn api(&self) -> &Api {
        &self.unit.api
    }

    pub fn kind(&self) -> CursorKind {
        // SAFETY: the cursor belongs to the live unit.
        CursorKind::from_raw(unsafe { (self.api().clang_getCursorKind)(self.raw) })
    }

    pub fn spelling(&self) -> String {
        // SAFETY: as above.
        unsafe { take_string(self.api(), (self.api().clang_getCursorSpelling)(self.raw)) }
    }

    pub fn children(&self) -> Vec<Cursor<'a>> {
        let mut raw: Vec<CxCursor> = Vec::new();
        // SAFETY: the visitor only pushes into the vector whose address is passed as the client data.
        unsafe {
            (self.api().clang_visitChildren)(self.raw, collect_child, &mut raw as *mut Vec<CxCursor> as *mut c_void);
        }
        raw.into_iter().map(|raw| Cursor { unit: self.unit, raw }).collect()
    }

    pub fn ty(&self) -> Ty<'a> {
        // SAFETY: as above.
        Ty { unit: self.unit, raw: unsafe { (self.api().clang_getCursorType)(self.raw) } }
    }

    pub fn result_type(&self) -> Ty<'a> {
        // SAFETY: as above.
        Ty { unit: self.unit, raw: unsafe { (self.api().clang_getCursorResultType)(self.raw) } }
    }

    pub fn typedef_underlying(&self) -> Ty<'a> {
        // SAFETY: as above.
        Ty { unit: self.unit, raw: unsafe { (self.api().clang_getTypedefDeclUnderlyingType)(self.raw) } }
    }

    pub fn enum_integer_type(&self) -> Ty<'a> {
        // SAFETY: as above.
        Ty { unit: self.unit, raw: unsafe { (self.api().clang_getEnumDeclIntegerType)(self.raw) } }
    }

    /// The value of an enumerator, read as unsigned when the enumeration's underlying type is unsigned so a value above
    /// `i64::MAX` keeps its meaning.
    pub fn enum_value(&self, unsigned: bool) -> i128 {
        // SAFETY: as above.
        unsafe {
            if unsigned {
                (self.api().clang_getEnumConstantDeclUnsignedValue)(self.raw) as i128
            } else {
                (self.api().clang_getEnumConstantDeclValue)(self.raw) as i128
            }
        }
    }

    pub fn arguments(&self) -> Vec<Cursor<'a>> {
        // SAFETY: as above.
        unsafe {
            let count = (self.api().clang_Cursor_getNumArguments)(self.raw);
            (0..count.max(0) as c_uint)
                .map(|index| Cursor { unit: self.unit, raw: (self.api().clang_Cursor_getArgument)(self.raw, index) })
                .collect()
        }
    }

    pub fn is_variadic(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_Cursor_isVariadic)(self.raw) != 0 }
    }

    pub fn is_definition(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_isCursorDefinition)(self.raw) != 0 }
    }

    /// The cursor of the definition this declaration belongs to, when the translation unit has one.
    pub fn definition(&self) -> Option<Cursor<'a>> {
        // SAFETY: as above. The null cursor has kind 70 (`CXCursor_FirstInvalid` and up).
        let raw = unsafe { (self.api().clang_getCursorDefinition)(self.raw) };
        let cursor = Cursor { unit: self.unit, raw };
        match cursor.kind() {
            CursorKind::Other(kind) if (70..=73).contains(&kind) => None,
            _ => Some(cursor),
        }
    }

    pub fn storage(&self) -> Storage {
        // SAFETY: as above.
        match unsafe { (self.api().clang_Cursor_getStorageClass)(self.raw) } {
            1 => Storage::None,
            2 => Storage::Extern,
            3 => Storage::Static,
            _ => Storage::Other,
        }
    }

    pub fn is_inline(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_Cursor_isFunctionInlined)(self.raw) != 0 }
    }

    pub fn is_bit_field(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_Cursor_isBitField)(self.raw) != 0 }
    }

    /// The offset of a field in bits, or `None` when libclang cannot tell (an incomplete or dependent type).
    pub fn field_offset_bits(&self) -> Option<u64> {
        // SAFETY: as above.
        let offset = unsafe { (self.api().clang_Cursor_getOffsetOfField)(self.raw) };
        u64::try_from(offset).ok()
    }

    pub fn is_function_like_macro(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_Cursor_isMacroFunctionLike)(self.raw) != 0 }
    }

    pub fn is_builtin_macro(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_Cursor_isMacroBuiltin)(self.raw) != 0 }
    }

    pub fn location(&self) -> Location {
        // SAFETY: as above; the out-parameters point at locals.
        unsafe {
            let location = (self.api().clang_getCursorLocation)(self.raw);
            let in_main_file = (self.api().clang_Location_isFromMainFile)(location) != 0;
            let mut file: *mut c_void = std::ptr::null_mut();
            let (mut line, mut column, mut offset) = (0, 0, 0);
            (self.api().clang_getFileLocation)(location, &mut file, &mut line, &mut column, &mut offset);
            let name = if file.is_null() {
                String::new()
            } else {
                take_string(self.api(), (self.api().clang_getFileName)(file))
            };
            Location { file: name, line, offset, in_main_file }
        }
    }

    /// The spelling of every token the cursor covers, for a macro definition: its name first, then its body.
    pub fn tokens(&self) -> Vec<Token> {
        // SAFETY: the token array comes from `clang_tokenize` and is released with `clang_disposeTokens`.
        unsafe {
            let range = (self.api().clang_getCursorExtent)(self.raw);
            let mut tokens: *mut CxToken = std::ptr::null_mut();
            let mut count: c_uint = 0;
            (self.api().clang_tokenize)(self.unit.unit, range, &mut tokens, &mut count);
            let mut found = Vec::new();
            if !tokens.is_null() {
                for index in 0..count as usize {
                    let token = *tokens.add(index);
                    let kind = match (self.api().clang_getTokenKind)(token) {
                        0 => TokenKind::Punctuation,
                        1 => TokenKind::Keyword,
                        2 => TokenKind::Identifier,
                        3 => TokenKind::Literal,
                        _ => TokenKind::Comment,
                    };
                    let text = take_string(self.api(), (self.api().clang_getTokenSpelling)(self.unit.unit, token));
                    found.push(Token { kind, text });
                }
                (self.api().clang_disposeTokens)(self.unit.unit, tokens, count);
            }
            found
        }
    }

    /// Evaluates a constant initializer, such as the value of `static const int N = 4`.
    pub fn evaluate(&self) -> Option<Evaluated> {
        // SAFETY: the result is released with `clang_EvalResult_dispose`.
        unsafe {
            let result = (self.api().clang_Cursor_Evaluate)(self.raw);
            if result.is_null() {
                return None;
            }
            let value = match (self.api().clang_EvalResult_getKind)(result) {
                1 => {
                    if (self.api().clang_EvalResult_isUnsignedInt)(result) != 0 {
                        Some(Evaluated::Int((self.api().clang_EvalResult_getAsUnsigned)(result) as i128))
                    } else {
                        Some(Evaluated::Int((self.api().clang_EvalResult_getAsLongLong)(result) as i128))
                    }
                }
                2 => Some(Evaluated::Float((self.api().clang_EvalResult_getAsDouble)(result))),
                _ => None,
            };
            (self.api().clang_EvalResult_dispose)(result);
            value
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Punctuation,
    Keyword,
    Identifier,
    Literal,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
}

/// The kinds of type the generator distinguishes (`CXTypeKind`); every other kind is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Invalid,
    Unexposed,
    Void,
    Bool,
    CharU,
    UChar,
    UShort,
    UInt,
    ULong,
    ULongLong,
    CharS,
    SChar,
    WChar,
    Short,
    Int,
    Long,
    LongLong,
    Float,
    Double,
    Pointer,
    Record,
    Enum,
    Typedef,
    FunctionNoProto,
    FunctionProto,
    ConstantArray,
    IncompleteArray,
    Elaborated,
    Other(i32),
}

impl TypeKind {
    fn from_raw(raw: c_int) -> TypeKind {
        match raw {
            0 => TypeKind::Invalid,
            1 => TypeKind::Unexposed,
            2 => TypeKind::Void,
            3 => TypeKind::Bool,
            4 => TypeKind::CharU,
            5 => TypeKind::UChar,
            8 => TypeKind::UShort,
            9 => TypeKind::UInt,
            10 => TypeKind::ULong,
            11 => TypeKind::ULongLong,
            13 => TypeKind::CharS,
            14 => TypeKind::SChar,
            15 => TypeKind::WChar,
            16 => TypeKind::Short,
            17 => TypeKind::Int,
            18 => TypeKind::Long,
            19 => TypeKind::LongLong,
            21 => TypeKind::Float,
            22 => TypeKind::Double,
            101 => TypeKind::Pointer,
            105 => TypeKind::Record,
            106 => TypeKind::Enum,
            107 => TypeKind::Typedef,
            110 => TypeKind::FunctionNoProto,
            111 => TypeKind::FunctionProto,
            112 => TypeKind::ConstantArray,
            114 => TypeKind::IncompleteArray,
            119 => TypeKind::Elaborated,
            other => TypeKind::Other(other),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Ty<'a> {
    unit: &'a TranslationUnit,
    raw: CxType,
}

impl<'a> Ty<'a> {
    fn api(&self) -> &Api {
        &self.unit.api
    }

    fn wrap(&self, raw: CxType) -> Ty<'a> {
        Ty { unit: self.unit, raw }
    }

    pub fn kind(&self) -> TypeKind {
        TypeKind::from_raw(self.raw.kind)
    }

    pub fn spelling(&self) -> String {
        // SAFETY: the type belongs to the live unit.
        unsafe { take_string(self.api(), (self.api().clang_getTypeSpelling)(self.raw)) }
    }

    pub fn canonical(&self) -> Ty<'a> {
        // SAFETY: as above.
        self.wrap(unsafe { (self.api().clang_getCanonicalType)(self.raw) })
    }

    pub fn pointee(&self) -> Ty<'a> {
        // SAFETY: as above.
        self.wrap(unsafe { (self.api().clang_getPointeeType)(self.raw) })
    }

    /// The type an elaborated type (`struct Foo`, `enum Bar`) names.
    pub fn named(&self) -> Ty<'a> {
        // SAFETY: as above.
        self.wrap(unsafe { (self.api().clang_Type_getNamedType)(self.raw) })
    }

    pub fn array_element(&self) -> Ty<'a> {
        // SAFETY: as above.
        self.wrap(unsafe { (self.api().clang_getArrayElementType)(self.raw) })
    }

    pub fn array_len(&self) -> Option<u64> {
        // SAFETY: as above.
        u64::try_from(unsafe { (self.api().clang_getArraySize)(self.raw) }).ok()
    }

    pub fn result(&self) -> Ty<'a> {
        // SAFETY: as above.
        self.wrap(unsafe { (self.api().clang_getResultType)(self.raw) })
    }

    pub fn arg_types(&self) -> Vec<Ty<'a>> {
        // SAFETY: as above.
        unsafe {
            let count = (self.api().clang_getNumArgTypes)(self.raw);
            (0..count.max(0) as c_uint).map(|index| self.wrap((self.api().clang_getArgType)(self.raw, index))).collect()
        }
    }

    pub fn is_variadic(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.api().clang_isFunctionTypeVariadic)(self.raw) != 0 }
    }

    /// The declaration of a typedef, struct, union, or enum type.
    pub fn declaration(&self) -> Cursor<'a> {
        // SAFETY: as above.
        Cursor { unit: self.unit, raw: unsafe { (self.api().clang_getTypeDeclaration)(self.raw) } }
    }

    /// The size in bytes, or `None` for an incomplete or dependent type.
    pub fn size(&self) -> Option<u64> {
        // SAFETY: as above.
        u64::try_from(unsafe { (self.api().clang_Type_getSizeOf)(self.raw) }).ok()
    }

    pub fn align(&self) -> Option<u64> {
        // SAFETY: as above.
        u64::try_from(unsafe { (self.api().clang_Type_getAlignOf)(self.raw) }).ok()
    }
}
