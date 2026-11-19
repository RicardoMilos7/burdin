//! Raw terminal input and output for the interactive REPL (`spec/repl.md`, RP11-RP13): one key at a time, so the
//! line editor owns echo and the cursor. The pieces standard Rust does not expose are written here — `libc`'s
//! termios on Unix, the console API on Windows — so the REPL adds no external dependency. Everything restores the
//! terminal when it is dropped, including when the REPL panics.

use std::io;

/// One keypress, as far as the editor cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    /// Ctrl-C: abandon the line being typed.
    Interrupt,
    /// Ctrl-D.
    Eof,
    /// A key the editor has no use for (an unrecognized escape sequence, a function key).
    Ignored,
}

/// The terminal in raw mode: no canonical buffering, no echo, no signal processing. Dropping it restores the
/// previous settings.
pub(crate) struct Terminal {
    #[cfg(unix)]
    saved: libc::termios,
    #[cfg(windows)]
    saved_input: u32,
    #[cfg(windows)]
    saved_output: u32,
    #[cfg(windows)]
    input: *mut core::ffi::c_void,
    #[cfg(windows)]
    output: *mut core::ffi::c_void,
}

impl Terminal {
    /// Puts standard input into raw mode. `None` when that is not possible (not a terminal, or a console without
    /// the API): the caller then falls back to plain reading.
    pub(crate) fn new() -> Option<Terminal> {
        #[cfg(unix)]
        {
            // SAFETY: termios calls on fd 0 with a properly initialized struct.
            unsafe {
                let mut saved: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(0, &mut saved) != 0 {
                    return None;
                }
                let mut raw = saved;
                raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
                raw.c_iflag &= !(libc::IXON | libc::ICRNL);
                raw.c_cc[libc::VMIN] = 1;
                raw.c_cc[libc::VTIME] = 0;
                if libc::tcsetattr(0, libc::TCSADRAIN, &raw) != 0 {
                    return None;
                }
                Some(Terminal { saved })
            }
        }
        #[cfg(windows)]
        {
            console::new_terminal()
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    /// Reads one key. `None` at the end of input or on a read error.
    pub(crate) fn read_key(&mut self) -> Option<Key> {
        #[cfg(unix)]
        {
            read_key_unix()
        }
        #[cfg(windows)]
        {
            console::read_key(self.input)
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    /// Restores the terminal early (the REPL also restores it by dropping the value).
    pub(crate) fn restore(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: restoring the settings this value saved.
            unsafe {
                libc::tcsetattr(0, libc::TCSADRAIN, &self.saved);
            }
        }
        #[cfg(windows)]
        {
            console::restore(self.input, self.saved_input);
            console::restore(self.output, self.saved_output);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(unix)]
fn read_key_unix() -> Option<Key> {
    use std::io::Read;
    let mut stdin = io::stdin().lock();
    loop {
        let mut byte = [0u8; 1];
        if stdin.read_exact(&mut byte).is_err() {
            return None;
        }
        match byte[0] {
            b'\r' | b'\n' => return Some(Key::Enter),
            b'\t' => return Some(Key::Tab),
            0x7f | 0x08 => return Some(Key::Backspace),
            0x03 => return Some(Key::Interrupt),
            0x04 => return Some(Key::Eof),
            0x1b => return read_escape(&mut stdin),
            b if b < 0x20 => continue,
            b => {
                // A UTF-8 sequence: take its continuation bytes.
                let need = match b {
                    0x00..=0x7f => 0,
                    0xc0..=0xdf => 1,
                    0xe0..=0xef => 2,
                    _ => 3,
                };
                let mut bytes = vec![b];
                for _ in 0..need {
                    let mut next = [0u8; 1];
                    if stdin.read_exact(&mut next).is_err() {
                        return None;
                    }
                    bytes.push(next[0]);
                }
                match std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()) {
                    Some(c) => return Some(Key::Char(c)),
                    None => continue,
                }
            }
        }
    }
}

/// An escape sequence after ESC: the arrows, Home, End, and Delete in both the CSI and SS3 spellings.
#[cfg(unix)]
fn read_escape(stdin: &mut impl std::io::Read) -> Option<Key> {
    let mut first = [0u8; 1];
    if stdin.read_exact(&mut first).is_err() {
        return None;
    }
    match first[0] {
        b'[' => {
            let mut params = [0u8; 1];
            if stdin.read_exact(&mut params).is_err() {
                return None;
            }
            let key = match params[0] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                b'1' | b'7' => read_final(stdin, Key::Home),
                b'8' => read_final(stdin, Key::End),
                b'4' => read_final(stdin, Key::End),
                b'3' => read_final(stdin, Key::Delete),
                _ => return Some(Key::Ignored),
            };
            Some(key)
        }
        b'O' => {
            let mut last = [0u8; 1];
            if stdin.read_exact(&mut last).is_err() {
                return None;
            }
            Some(match last[0] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _ => Key::Ignored,
            })
        }
        _ => Some(Key::Ignored),
    }
}

/// Reads the `~` that ends a `ESC [ n ~` sequence and returns `key`.
#[cfg(unix)]
fn read_final(stdin: &mut impl std::io::Read, key: Key) -> Key {
    let mut tilde = [0u8; 1];
    let _ = stdin.read_exact(&mut tilde);
    key
}

#[cfg(windows)]
mod console {
    //! The Windows console: raw key events from `ReadConsoleInputW`, and virtual-terminal output.

    use super::{Key, Terminal};

    type Handle = *mut core::ffi::c_void;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    const KEY_EVENT: u16 = 0x0001;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyEventRecord {
        key_down: i32,
        repeat_count: u16,
        virtual_key_code: u16,
        virtual_scan_code: u16,
        unicode: u16,
        control_key_state: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    union Event {
        key: KeyEventRecord,
        raw: [u8; 16],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputRecord {
        event_type: u16,
        padding: u16,
        event: Event,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> Handle;
        fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
        fn ReadConsoleInputW(handle: Handle, records: *mut InputRecord, length: u32, read: *mut u32) -> i32;
    }

    pub(super) fn new_terminal() -> Option<Terminal> {
        // SAFETY: the console API with handles from GetStdHandle; every failure falls back to plain reading.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            if input.is_null() || output.is_null() {
                return None;
            }
            let (mut saved_input, mut saved_output) = (0u32, 0u32);
            if GetConsoleMode(input, &mut saved_input) == 0 || GetConsoleMode(output, &mut saved_output) == 0 {
                return None;
            }
            let raw = saved_input & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
            if SetConsoleMode(input, raw) == 0 {
                return None;
            }
            // ANSI escapes need virtual-terminal output.
            if SetConsoleMode(output, saved_output | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0 {
                SetConsoleMode(input, saved_input);
                return None;
            }
            Some(Terminal { saved_input, saved_output, input, output })
        }
    }

    pub(super) fn read_key(input: Handle) -> Option<Key> {
        loop {
            let mut record = InputRecord { event_type: 0, padding: 0, event: Event { raw: [0; 16] } };
            let mut read = 0u32;
            // SAFETY: one record buffer, one handle from the constructor.
            let ok = unsafe { ReadConsoleInputW(input, &mut record, 1, &mut read) };
            if ok == 0 || read == 0 {
                return None;
            }
            if record.event_type != KEY_EVENT {
                continue;
            }
            // SAFETY: the event type says the union holds a key event.
            let key: KeyEventRecord = unsafe { record.event.key };
            if key.key_down == 0 {
                continue;
            }
            if let Some(key) = map_key(&key) {
                return Some(key);
            }
        }
    }

    fn map_key(key: &KeyEventRecord) -> Option<Key> {
        match key.virtual_key_code {
            0x0d => Some(Key::Enter),
            0x09 => Some(Key::Tab),
            0x08 => Some(Key::Backspace),
            0x25 => Some(Key::Left),
            0x27 => Some(Key::Right),
            0x26 => Some(Key::Up),
            0x28 => Some(Key::Down),
            0x24 => Some(Key::Home),
            0x23 => Some(Key::End),
            0x2e => Some(Key::Delete),
            _ => {
                let c = char::from_u32(u32::from(key.unicode))?;
                match c {
                    '\u{3}' => Some(Key::Interrupt),
                    '\u{4}' => Some(Key::Eof),
                    '\r' | '\n' => Some(Key::Enter),
                    '\u{0}' => None,
                    c if (c as u32) < 0x20 => None,
                    c => Some(Key::Char(c)),
                }
            }
        }
    }

    pub(super) fn restore(handle: Handle, mode: u32) {
        // SAFETY: restoring a mode this process saved.
        unsafe {
            SetConsoleMode(handle, mode);
        }
    }
}
