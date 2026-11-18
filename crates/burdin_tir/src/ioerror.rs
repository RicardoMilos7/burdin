//! The mapping from an operating system's error code to `std.IoErrorKind` (`spec/errors.md`, ERR11).
//!
//! The compiler is not a cross-compiler (`target_os`), so exactly one of these tables is the program's. Each
//! entry pairs the platform's own code with the *name* of a kind: names, not variant indices, so reordering
//! `std.IoErrorKind` cannot silently change a classification. A code no table lists is `Other`.
//!
//! On Unix the code is `errno` and the numbers are the host C library's; the unit tests check the table that
//! belongs to the host against `libc`'s constants. On Windows it is the Win32 error code (`GetLastError`), which
//! the CRT also leaves in `_doserrno` for the file functions, so both sources agree.

/// The operating system a table describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

/// The portable kinds, in the order `std.IoErrorKind` declares them (`spec/errors.md`, ERR11).
pub const KINDS: [&str; 22] = [
    "NotFound",
    "PermissionDenied",
    "AlreadyExists",
    "IsADirectory",
    "NotADirectory",
    "DirectoryNotEmpty",
    "InvalidInput",
    "InvalidData",
    "UnexpectedEof",
    "Interrupted",
    "BrokenPipe",
    "WouldBlock",
    "TimedOut",
    "ConnectionRefused",
    "ConnectionReset",
    "ConnectionAborted",
    "NotConnected",
    "AddrInUse",
    "AddrNotAvailable",
    "NetworkUnreachable",
    "HostUnreachable",
    "Other",
];

/// The platform of the code the program's primitives report: the host's, because the compiler does not
/// cross-compile.
pub fn host_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

/// The platform a target triple names.
pub fn platform(windows: bool, darwin: bool) -> Platform {
    if windows {
        Platform::Windows
    } else if darwin {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

/// The codes of `platform` and the kind each one means, in ascending code order. `Other` is not listed: it is
/// the fallback for every code that is not here.
pub fn table(platform: Platform) -> &'static [(i32, &'static str)] {
    match platform {
        Platform::Linux => LINUX,
        Platform::MacOs => MACOS,
        Platform::Windows => WINDOWS,
    }
}

/// The kind `code` means on `platform`.
pub fn kind_name(platform: Platform, code: i32) -> &'static str {
    table(platform).iter().find(|(c, _)| *c == code).map_or("Other", |(_, name)| *name)
}

/// Linux's `errno` values. The comment on each entry names the `errno` constant.
const LINUX: &[(i32, &str)] = &[
    (1, "PermissionDenied"),     // EPERM
    (2, "NotFound"),             // ENOENT
    (4, "Interrupted"),          // EINTR
    (8, "InvalidData"),          // ENOEXEC
    (11, "WouldBlock"),          // EAGAIN (and EWOULDBLOCK)
    (13, "PermissionDenied"),    // EACCES
    (17, "AlreadyExists"),       // EEXIST
    (20, "NotADirectory"),       // ENOTDIR
    (21, "IsADirectory"),        // EISDIR
    (22, "InvalidInput"),        // EINVAL
    (30, "PermissionDenied"),    // EROFS
    (32, "BrokenPipe"),          // EPIPE
    (36, "InvalidInput"),        // ENAMETOOLONG
    (39, "DirectoryNotEmpty"),   // ENOTEMPTY
    (40, "InvalidInput"),        // ELOOP
    (84, "InvalidData"),         // EILSEQ
    (98, "AddrInUse"),           // EADDRINUSE
    (99, "AddrNotAvailable"),    // EADDRNOTAVAIL
    (101, "NetworkUnreachable"), // ENETUNREACH
    (103, "ConnectionAborted"),  // ECONNABORTED
    (104, "ConnectionReset"),    // ECONNRESET
    (107, "NotConnected"),       // ENOTCONN
    (110, "TimedOut"),           // ETIMEDOUT
    (111, "ConnectionRefused"),  // ECONNREFUSED
    (113, "HostUnreachable"),    // EHOSTUNREACH
];

/// macOS's `errno` values. Several differ from Linux's, so the two tables cannot be shared.
const MACOS: &[(i32, &str)] = &[
    (1, "PermissionDenied"),    // EPERM
    (2, "NotFound"),            // ENOENT
    (4, "Interrupted"),         // EINTR
    (8, "InvalidData"),         // ENOEXEC
    (13, "PermissionDenied"),   // EACCES
    (17, "AlreadyExists"),      // EEXIST
    (20, "NotADirectory"),      // ENOTDIR
    (21, "IsADirectory"),       // EISDIR
    (22, "InvalidInput"),       // EINVAL
    (30, "PermissionDenied"),   // EROFS
    (32, "BrokenPipe"),         // EPIPE
    (35, "WouldBlock"),         // EAGAIN (and EWOULDBLOCK)
    (48, "AddrInUse"),          // EADDRINUSE
    (49, "AddrNotAvailable"),   // EADDRNOTAVAIL
    (51, "NetworkUnreachable"), // ENETUNREACH
    (53, "ConnectionAborted"),  // ECONNABORTED
    (54, "ConnectionReset"),    // ECONNRESET
    (57, "NotConnected"),       // ENOTCONN
    (60, "TimedOut"),           // ETIMEDOUT
    (61, "ConnectionRefused"),  // ECONNREFUSED
    (62, "InvalidInput"),       // ELOOP
    (63, "InvalidInput"),       // ENAMETOOLONG
    (65, "HostUnreachable"),    // EHOSTUNREACH
    (66, "DirectoryNotEmpty"),  // ENOTEMPTY
    (92, "InvalidData"),        // EILSEQ
];

/// The Windows error codes (`GetLastError`) the runtime can meet. Windows has no code for "is a directory";
/// removing one is `ERROR_ACCESS_DENIED`, so that failure is `PermissionDenied` there.
const WINDOWS: &[(i32, &str)] = &[
    (2, "NotFound"),               // ERROR_FILE_NOT_FOUND
    (3, "NotFound"),               // ERROR_PATH_NOT_FOUND
    (5, "PermissionDenied"),       // ERROR_ACCESS_DENIED
    (13, "InvalidData"),           // ERROR_INVALID_DATA
    (15, "NotFound"),              // ERROR_INVALID_DRIVE
    (19, "PermissionDenied"),      // ERROR_WRITE_PROTECT
    (23, "InvalidData"),           // ERROR_CRC
    (32, "PermissionDenied"),      // ERROR_SHARING_VIOLATION
    (33, "PermissionDenied"),      // ERROR_LOCK_VIOLATION
    (38, "UnexpectedEof"),         // ERROR_HANDLE_EOF
    (80, "AlreadyExists"),         // ERROR_FILE_EXISTS
    (87, "InvalidInput"),          // ERROR_INVALID_PARAMETER
    (109, "BrokenPipe"),           // ERROR_BROKEN_PIPE
    (121, "TimedOut"),             // ERROR_SEM_TIMEOUT
    (123, "InvalidInput"),         // ERROR_INVALID_NAME
    (145, "DirectoryNotEmpty"),    // ERROR_DIR_NOT_EMPTY
    (183, "AlreadyExists"),        // ERROR_ALREADY_EXISTS
    (206, "InvalidInput"),         // ERROR_FILENAME_EXCED_RANGE
    (232, "BrokenPipe"),           // ERROR_NO_DATA
    (267, "NotADirectory"),        // ERROR_DIRECTORY
    (995, "Interrupted"),          // ERROR_OPERATION_ABORTED
    (997, "WouldBlock"),           // ERROR_IO_PENDING
    (1460, "TimedOut"),            // ERROR_TIMEOUT
    (10048, "AddrInUse"),          // WSAEADDRINUSE
    (10049, "AddrNotAvailable"),   // WSAEADDRNOTAVAIL
    (10051, "NetworkUnreachable"), // WSAENETUNREACH
    (10053, "ConnectionAborted"),  // WSAECONNABORTED
    (10054, "ConnectionReset"),    // WSAECONNRESET
    (10057, "NotConnected"),       // WSAENOTCONN
    (10060, "TimedOut"),           // WSAETIMEDOUT
    (10061, "ConnectionRefused"),  // WSAECONNREFUSED
    (10065, "HostUnreachable"),    // WSAEHOSTUNREACH
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `std/src/std.bd`'s `enum IoErrorKind` block, as source text.
    const STD: &str = include_str!("../../../std/src/std.bd");

    /// The variant names of `std.IoErrorKind`, in declaration order.
    fn declared_kinds() -> Vec<String> {
        let start = STD.find("enum IoErrorKind").expect("std declares IoErrorKind");
        let body = &STD[start..];
        let open = body.find('{').expect("the enum has a body");
        let close = body.find('}').expect("the enum body ends");
        body[open + 1..close]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//"))
            .map(|line| line.split_whitespace().next().expect("a variant name").to_string())
            .collect()
    }

    #[test]
    fn kinds_match_the_std_declaration() {
        assert_eq!(declared_kinds(), KINDS, "KINDS must list IoErrorKind's variants in declaration order");
    }

    #[test]
    fn every_table_names_a_real_kind_and_no_code_twice() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let mut seen = Vec::new();
            for (code, name) in table(platform) {
                assert!(KINDS.contains(name), "{platform:?}: {code} names the unknown kind {name}");
                assert!(!seen.contains(code), "{platform:?}: code {code} is listed twice");
                seen.push(*code);
            }
            let mut sorted = seen.clone();
            sorted.sort_unstable();
            assert_eq!(seen, sorted, "{platform:?}: the table is not in ascending code order");
        }
    }

    #[test]
    fn unknown_codes_are_other() {
        assert_eq!(kind_name(Platform::Linux, 987_654), "Other");
        assert_eq!(kind_name(Platform::MacOs, 987_654), "Other");
        assert_eq!(kind_name(Platform::Windows, 987_654), "Other");
        assert_eq!(kind_name(Platform::Windows, 0), "Other");
    }

    #[test]
    fn the_shared_codes_agree_across_platforms() {
        // A missing file, an entry that already exists, and a name that is not a directory have the same
        // meaning on all three platforms the compiler supports.
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert_eq!(kind_name(platform, 2), "NotFound", "{platform:?}");
        }
        assert_eq!(kind_name(Platform::Linux, 17), "AlreadyExists");
        assert_eq!(kind_name(Platform::MacOs, 17), "AlreadyExists");
        assert_eq!(kind_name(Platform::Windows, 183), "AlreadyExists");
        assert_eq!(kind_name(Platform::Linux, 20), "NotADirectory");
        assert_eq!(kind_name(Platform::MacOs, 20), "NotADirectory");
        assert_eq!(kind_name(Platform::Windows, 267), "NotADirectory");
    }

    #[test]
    fn windows_maps_the_codes_the_runtime_meets() {
        assert_eq!(kind_name(Platform::Windows, 3), "NotFound"); // ERROR_PATH_NOT_FOUND
        assert_eq!(kind_name(Platform::Windows, 5), "PermissionDenied"); // ERROR_ACCESS_DENIED
        assert_eq!(kind_name(Platform::Windows, 145), "DirectoryNotEmpty"); // ERROR_DIR_NOT_EMPTY
        assert_eq!(kind_name(Platform::Windows, 38), "UnexpectedEof"); // ERROR_HANDLE_EOF
        assert_eq!(kind_name(Platform::Windows, 13), "InvalidData"); // ERROR_INVALID_DATA
    }

    /// The Linux table against the host library's constants, so a mistyped number fails here instead of on a
    /// user's machine.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_codes_match_libc() {
        let expected = [
            (libc::EPERM, "PermissionDenied"),
            (libc::ENOENT, "NotFound"),
            (libc::EINTR, "Interrupted"),
            (libc::ENOEXEC, "InvalidData"),
            (libc::EAGAIN, "WouldBlock"),
            (libc::EACCES, "PermissionDenied"),
            (libc::EEXIST, "AlreadyExists"),
            (libc::ENOTDIR, "NotADirectory"),
            (libc::EISDIR, "IsADirectory"),
            (libc::EINVAL, "InvalidInput"),
            (libc::EROFS, "PermissionDenied"),
            (libc::EPIPE, "BrokenPipe"),
            (libc::ENAMETOOLONG, "InvalidInput"),
            (libc::ENOTEMPTY, "DirectoryNotEmpty"),
            (libc::ELOOP, "InvalidInput"),
            (libc::EILSEQ, "InvalidData"),
            (libc::EADDRINUSE, "AddrInUse"),
            (libc::EADDRNOTAVAIL, "AddrNotAvailable"),
            (libc::ENETUNREACH, "NetworkUnreachable"),
            (libc::ECONNABORTED, "ConnectionAborted"),
            (libc::ECONNRESET, "ConnectionReset"),
            (libc::ENOTCONN, "NotConnected"),
            (libc::ETIMEDOUT, "TimedOut"),
            (libc::ECONNREFUSED, "ConnectionRefused"),
            (libc::EHOSTUNREACH, "HostUnreachable"),
        ];
        assert_eq!(LINUX, &expected);
    }

    /// The macOS table against the host library's constants; several values differ from Linux's.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_codes_match_libc() {
        let expected = [
            (libc::EPERM, "PermissionDenied"),
            (libc::ENOENT, "NotFound"),
            (libc::EINTR, "Interrupted"),
            (libc::ENOEXEC, "InvalidData"),
            (libc::EACCES, "PermissionDenied"),
            (libc::EEXIST, "AlreadyExists"),
            (libc::ENOTDIR, "NotADirectory"),
            (libc::EISDIR, "IsADirectory"),
            (libc::EINVAL, "InvalidInput"),
            (libc::EROFS, "PermissionDenied"),
            (libc::EPIPE, "BrokenPipe"),
            (libc::EAGAIN, "WouldBlock"),
            (libc::EADDRINUSE, "AddrInUse"),
            (libc::EADDRNOTAVAIL, "AddrNotAvailable"),
            (libc::ENETUNREACH, "NetworkUnreachable"),
            (libc::ECONNABORTED, "ConnectionAborted"),
            (libc::ECONNRESET, "ConnectionReset"),
            (libc::ENOTCONN, "NotConnected"),
            (libc::ETIMEDOUT, "TimedOut"),
            (libc::ECONNREFUSED, "ConnectionRefused"),
            (libc::ELOOP, "InvalidInput"),
            (libc::ENAMETOOLONG, "InvalidInput"),
            (libc::EHOSTUNREACH, "HostUnreachable"),
            (libc::ENOTEMPTY, "DirectoryNotEmpty"),
            (libc::EILSEQ, "InvalidData"),
        ];
        assert_eq!(MACOS, &expected);
    }
}
