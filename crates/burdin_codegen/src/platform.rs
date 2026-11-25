//! The system constants the generated runtime bakes in: the offsets of the C structures it reads (`struct stat`,
//! `struct dirent`, `struct addrinfo`), the socket option numbers, and the `errno` values it treats as "try again".
//!
//! They belong to the platform being compiled for, not to the machine the compiler runs on, or `--target` could not
//! produce a correct program (`spec/cross-compilation.md`, XC5). Each platform has one table below; the unit tests
//! check the table of the host against the host's own C headers (through the `libc` crate), which is the only
//! platform they can check against. The tables for other platforms follow the same headers, so a wrong number there
//! is a bug the cross-compilation conformance programs cannot see until they run on that platform.

/// The system constants of one platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SysLayout {
    /// The offset of `d_name` in a `struct dirent`. Windows has no `dirent`; its listing reads `WIN32_FIND_DATAA`.
    pub dirent_name_offset: u64,
    /// The size of a `struct stat`, in whole 8-byte words, so the buffer is 8-byte aligned.
    pub stat_words: u32,
    /// The fields the runtime reads from `struct stat`.
    pub stat_size_offset: u64,
    pub stat_mode_offset: u64,
    pub stat_atime_offset: u64,
    pub stat_mtime_offset: u64,
    /// `st_mode` is 16 bits on macOS and Windows, and 32 on Linux.
    pub stat_mode_bytes: u32,
    /// The fields of `struct addrinfo` the resolver reads, and its size.
    pub addrinfo_family_offset: u64,
    pub addrinfo_addr_offset: u64,
    pub addrinfo_next_offset: u64,
    pub addrinfo_size: u64,
    pub sol_socket: i32,
    pub so_error: i32,
    pub so_reuseaddr: i32,
    pub tcp_nodelay: i32,
    /// `send`'s flag that turns a write to a peer that has gone away into `EPIPE` instead of a `SIGPIPE`. Only Linux
    /// has it; macOS takes `SO_NOSIGPIPE` on the socket instead, and Windows raises no signal.
    pub msg_nosignal: i32,
    /// `fcntl`'s "get the file status flags", "set them", and the non-blocking bit among them (Unix only; the
    /// Windows values are placeholders for a branch it never runs).
    pub f_getfl: i32,
    pub f_setfl: i32,
    pub o_nonblock: i32,
    /// The error numbers that mean "try again": a non-blocking call would block, a connect is still in progress, or a
    /// signal interrupted the call. On Windows these are the Winsock codes `WSAGetLastError` reports.
    pub retry_codes: &'static [i32],
}

impl SysLayout {
    /// The layout of the platform named by the triple's parts.
    pub(crate) fn new(windows: bool, darwin: bool, aarch64: bool) -> SysLayout {
        if windows {
            // `_stat64` and `WIN32_FIND_DATAA` are the same on x86-64 and arm64.
            SysLayout {
                dirent_name_offset: 0,
                stat_words: 7,
                stat_size_offset: 24,
                stat_mode_offset: 6,
                stat_atime_offset: 32,
                stat_mtime_offset: 40,
                stat_mode_bytes: 2,
                addrinfo_family_offset: 4,
                addrinfo_addr_offset: 32,
                addrinfo_next_offset: 40,
                addrinfo_size: 48,
                sol_socket: 0xffff,
                so_error: 0x1007,
                so_reuseaddr: 0x0004,
                tcp_nodelay: 1,
                msg_nosignal: 0,
                f_getfl: 3,
                f_setfl: 4,
                o_nonblock: 0x800,
                retry_codes: &[10035, 10036, 10004, 10037, 10055],
            }
        } else if darwin {
            // The 64-bit-inode `struct stat` and `struct dirent`, the same on Intel and Apple silicon.
            SysLayout {
                dirent_name_offset: 21,
                stat_words: 18,
                stat_size_offset: 96,
                stat_mode_offset: 4,
                stat_atime_offset: 32,
                stat_mtime_offset: 48,
                stat_mode_bytes: 2,
                addrinfo_family_offset: 4,
                addrinfo_addr_offset: 32,
                addrinfo_next_offset: 40,
                addrinfo_size: 48,
                sol_socket: 0xffff,
                so_error: 0x1007,
                so_reuseaddr: 0x0004,
                tcp_nodelay: 1,
                msg_nosignal: 0,
                f_getfl: 3,
                f_setfl: 4,
                o_nonblock: 0x4,
                retry_codes: &[35, 35, 36, 4],
            }
        } else {
            // glibc. `struct stat` differs between x86-64 (144 bytes) and arm64 (128 bytes); the rest is the same.
            let (stat_words, stat_mode_offset) = if aarch64 { (16, 16) } else { (18, 24) };
            SysLayout {
                dirent_name_offset: 19,
                stat_words,
                stat_size_offset: 48,
                stat_mode_offset,
                stat_atime_offset: 72,
                stat_mtime_offset: 88,
                stat_mode_bytes: 4,
                addrinfo_family_offset: 4,
                addrinfo_addr_offset: 24,
                addrinfo_next_offset: 40,
                addrinfo_size: 48,
                sol_socket: 1,
                so_error: 4,
                so_reuseaddr: 2,
                tcp_nodelay: 1,
                msg_nosignal: 0x4000,
                f_getfl: 3,
                f_setfl: 4,
                o_nonblock: 0x800,
                retry_codes: &[11, 11, 115, 4],
            }
        }
    }

    /// The layout of the machine the compiler runs on.
    #[cfg(test)]
    pub(crate) fn host() -> SysLayout {
        SysLayout::new(cfg!(windows), cfg!(target_os = "macos"), cfg!(target_arch = "aarch64"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every number of the host's table is what the host's C headers say.
    #[cfg(unix)]
    #[test]
    fn the_host_layout_is_the_host_c_layout() {
        let l = SysLayout::host();
        assert_eq!(l.dirent_name_offset, std::mem::offset_of!(libc::dirent, d_name) as u64);
        assert_eq!(l.stat_words, (std::mem::size_of::<libc::stat>() as u32).div_ceil(8));
        assert_eq!(l.stat_size_offset, std::mem::offset_of!(libc::stat, st_size) as u64);
        assert_eq!(l.stat_mode_offset, std::mem::offset_of!(libc::stat, st_mode) as u64);
        assert_eq!(l.stat_atime_offset, std::mem::offset_of!(libc::stat, st_atime) as u64);
        assert_eq!(l.stat_mtime_offset, std::mem::offset_of!(libc::stat, st_mtime) as u64);
        let mode_bytes = std::mem::size_of::<libc::mode_t>();
        assert_eq!(l.stat_mode_bytes as usize, mode_bytes);
        assert_eq!(l.addrinfo_family_offset, std::mem::offset_of!(libc::addrinfo, ai_family) as u64);
        assert_eq!(l.addrinfo_addr_offset, std::mem::offset_of!(libc::addrinfo, ai_addr) as u64);
        assert_eq!(l.addrinfo_next_offset, std::mem::offset_of!(libc::addrinfo, ai_next) as u64);
        assert_eq!(l.addrinfo_size, std::mem::size_of::<libc::addrinfo>() as u64);
        assert_eq!(l.sol_socket, libc::SOL_SOCKET);
        assert_eq!(l.so_error, libc::SO_ERROR);
        assert_eq!(l.so_reuseaddr, libc::SO_REUSEADDR);
        assert_eq!(l.tcp_nodelay, libc::TCP_NODELAY);
        assert_eq!(l.f_getfl, libc::F_GETFL);
        assert_eq!(l.f_setfl, libc::F_SETFL);
        assert_eq!(l.o_nonblock, libc::O_NONBLOCK);
        assert_eq!(l.retry_codes, [libc::EAGAIN, libc::EWOULDBLOCK, libc::EINPROGRESS, libc::EINTR]);
        #[cfg(target_os = "linux")]
        assert_eq!(l.msg_nosignal, libc::MSG_NOSIGNAL);
    }

    #[cfg(windows)]
    #[test]
    fn the_host_layout_is_the_host_c_layout() {
        let l = SysLayout::host();
        assert_eq!(l.stat_words, (std::mem::size_of::<libc::stat>() as u32).div_ceil(8));
        assert_eq!(l.stat_size_offset, std::mem::offset_of!(libc::stat, st_size) as u64);
        assert_eq!(l.stat_mode_offset, std::mem::offset_of!(libc::stat, st_mode) as u64);
        assert_eq!(l.stat_atime_offset, std::mem::offset_of!(libc::stat, st_atime) as u64);
        assert_eq!(l.stat_mtime_offset, std::mem::offset_of!(libc::stat, st_mtime) as u64);
    }

    /// The tables differ where the platforms do, and only there.
    #[test]
    fn the_platforms_differ_where_their_headers_do() {
        let x86 = SysLayout::new(false, false, false);
        let arm = SysLayout::new(false, false, true);
        assert_ne!(x86.stat_mode_offset, arm.stat_mode_offset);
        let same = |l: SysLayout| SysLayout { stat_words: 0, stat_mode_offset: 0, ..l };
        assert_eq!(same(x86), same(arm));
        let mac_x86 = SysLayout::new(false, true, false);
        assert_eq!(mac_x86, SysLayout::new(false, true, true));
        assert_ne!(mac_x86.sol_socket, x86.sol_socket);
        assert_eq!(SysLayout::new(true, false, false), SysLayout::new(true, false, true));
    }
}
