//! The interactive line editor (`spec/repl.md`, RP11-RP13): input line by line with history, syntax highlighting
//! with the editor grammar's token classes, and tab completion through the query library. Multi-line input is not
//! this module's business: the REPL asks the parser whether the input is complete and calls `read` again with the
//! continuation prompt.

use crate::highlight;
use crate::terminal::{Key, Terminal};
use std::io::Write;
use std::path::PathBuf;

/// What reading one line came to.
pub(crate) enum Read {
    /// A line, without its newline.
    Line(String),
    /// Ctrl-C: the caller abandons whatever was being typed.
    Interrupted,
    /// Ctrl-D on an empty line, or the end of input.
    Eof,
}

/// The editor: the terminal in raw mode and the history.
pub(crate) struct Editor {
    terminal: Terminal,
    history: History,
}

impl Editor {
    /// Builds an editor when standard input can be a raw terminal. `history_file` is where the history lives;
    /// `None` when the Burdin home does not resolve (the session then runs without a persisted history).
    pub(crate) fn new(history_file: Option<PathBuf>) -> Option<Editor> {
        let terminal = Terminal::new()?;
        Some(Editor { terminal, history: History::load(history_file) })
    }

    /// Reads one line under `prompt`, with completion from `complete` (given the text before the cursor and the
    /// word being typed).
    pub(crate) fn read(&mut self, prompt: &str, complete: &mut dyn FnMut(&str, &str) -> Vec<String>) -> Read {
        let mut buf: Vec<char> = Vec::new();
        let mut cursor = 0usize;
        let mut draft: Option<Vec<char>> = None;
        self.history.reset();
        loop {
            self.draw(prompt, &buf, cursor);
            let Some(key) = self.terminal.read_key() else { return Read::Eof };
            match key {
                Key::Ignored => {}
                Key::Char(c) => {
                    buf.insert(cursor, c);
                    cursor += 1;
                }
                Key::Enter => {
                    self.newline();
                    let line: String = buf.iter().collect();
                    self.history.push(&line);
                    return Read::Line(line);
                }
                Key::Backspace => {
                    if cursor > 0 {
                        cursor -= 1;
                        buf.remove(cursor);
                    }
                }
                Key::Delete => {
                    if cursor < buf.len() {
                        buf.remove(cursor);
                    }
                }
                Key::Left => cursor = cursor.saturating_sub(1),
                Key::Right => cursor = (cursor + 1).min(buf.len()),
                Key::Home => cursor = 0,
                Key::End => cursor = buf.len(),
                Key::Up => {
                    if draft.is_none() {
                        draft = Some(buf.clone());
                    }
                    if let Some(entry) = self.history.older() {
                        buf = entry.chars().collect();
                        cursor = buf.len();
                    }
                }
                Key::Down => match self.history.newer() {
                    Some(entry) => {
                        buf = entry.chars().collect();
                        cursor = buf.len();
                    }
                    None => {
                        if let Some(d) = draft.take() {
                            buf = d;
                            cursor = buf.len();
                        }
                    }
                },
                Key::Tab => {
                    let before: String = buf[..cursor].iter().collect();
                    let prefix = word_at(&before);
                    let candidates = complete(&before, &prefix);
                    match candidates.len() {
                        0 => {}
                        1 => {
                            let rest = candidates[0].strip_prefix(&prefix).unwrap_or("").to_string();
                            for c in rest.chars() {
                                buf.insert(cursor, c);
                                cursor += 1;
                            }
                        }
                        _ => self.show_candidates(prompt, &buf, cursor, &candidates),
                    }
                }
                Key::Interrupt => {
                    self.newline();
                    return Read::Interrupted;
                }
                Key::Eof => {
                    if buf.is_empty() {
                        self.newline();
                        return Read::Eof;
                    }
                    // Ctrl-D on a non-empty line deletes forward, as readline does.
                    if cursor < buf.len() {
                        buf.remove(cursor);
                    }
                }
            }
        }
    }

    /// Draws the prompt and the highlighted line, and moves the cursor to its place.
    fn draw(&mut self, prompt: &str, buf: &[char], cursor: usize) {
        let text: String = buf.iter().collect();
        let shown = highlight::line(&text);
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\r{prompt}{shown}\x1b[K");
        // The tail after the cursor is the number of characters to move back; the color codes are invisible.
        let tail = buf.len().saturating_sub(cursor);
        if tail > 0 {
            let _ = write!(out, "\x1b[{tail}D");
        }
        let _ = out.flush();
    }

    /// Ends the line being edited.
    fn newline(&mut self) {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\r\n");
        let _ = out.flush();
    }

    /// Prints the candidates below the line and redraws it.
    fn show_candidates(&mut self, prompt: &str, buf: &[char], cursor: usize, candidates: &[String]) {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\r\n");
        let width = candidates.iter().map(|c| c.chars().count()).max().unwrap_or(0) + 2;
        let columns = (80 / width.max(1)).max(1);
        for (index, candidate) in candidates.iter().enumerate() {
            let _ = write!(out, "{candidate:<width$}");
            if (index + 1) % columns == 0 {
                let _ = write!(out, "\r\n");
            }
        }
        if !candidates.len().is_multiple_of(columns) {
            let _ = write!(out, "\r\n");
        }
        let _ = out.flush();
        self.draw(prompt, buf, cursor);
    }

    /// Writes the history to its file, keeping the most recent entries.
    pub(crate) fn save_history(&mut self) {
        self.history.save();
    }
}

/// The word being typed at the end of `before`: its identifier characters.
fn word_at(before: &str) -> String {
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    word
}

/// At most this many entries are kept, so the file cannot grow without bound.
const MAX_HISTORY: usize = 1000;

/// The session's history: the lines entered so far, loaded from the Burdin home at start and written back at exit.
pub(crate) struct History {
    entries: Vec<String>,
    /// The position `older`/`newer` walk, counted from the end.
    position: usize,
    file: Option<PathBuf>,
}

impl History {
    fn load(file: Option<PathBuf>) -> History {
        let entries = file
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| text.lines().map(str::to_string).filter(|line| !line.is_empty()).collect())
            .unwrap_or_default();
        History { entries, position: 0, file }
    }

    fn reset(&mut self) {
        self.position = 0;
    }

    /// Records a line: an empty line, a repeat of the last line, and `:quit` are not worth keeping.
    fn push(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() || line == ":quit" {
            return;
        }
        if self.entries.last().is_some_and(|last| last == line) {
            return;
        }
        self.entries.push(line.to_string());
        if self.entries.len() > MAX_HISTORY {
            let excess = self.entries.len() - MAX_HISTORY;
            self.entries.drain(..excess);
        }
    }

    /// The previous entry, older toward the start of the file. The oldest entry repeats.
    fn older(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        self.position = (self.position + 1).min(self.entries.len());
        Some(self.entries[self.entries.len() - self.position].clone())
    }

    /// The next entry; `None` at the newest one (the caller then restores its draft).
    fn newer(&mut self) -> Option<String> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        if self.position == 0 {
            return None;
        }
        Some(self.entries[self.entries.len() - self.position].clone())
    }

    fn save(&mut self) {
        let Some(path) = &self.file else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text: String = self.entries.iter().map(|entry| format!("{entry}\n")).collect();
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> History {
        History { entries: entries.iter().map(|e| e.to_string()).collect(), position: 0, file: None }
    }

    #[test]
    fn the_word_at_the_cursor_is_the_identifier_run() {
        assert_eq!(word_at("let x = val"), "val");
        assert_eq!(word_at("s."), "");
        assert_eq!(word_at("pri"), "pri");
        assert_eq!(word_at("x + "), "");
    }

    #[test]
    fn history_walks_back_and_forth() {
        let mut history = history(&["one", "two", "three"]);
        assert_eq!(history.older().as_deref(), Some("three"));
        assert_eq!(history.older().as_deref(), Some("two"));
        assert_eq!(history.older().as_deref(), Some("one"));
        assert_eq!(history.older().as_deref(), Some("one"), "the oldest entry repeats");
        assert_eq!(history.newer().as_deref(), Some("two"));
        assert_eq!(history.newer().as_deref(), Some("three"));
        assert_eq!(history.newer(), None, "the newest entry gives way to the draft");
    }

    #[test]
    fn repeated_lines_are_not_recorded_twice() {
        let mut history = self::history(&[]);
        history.push("let x = 1");
        history.push("let x = 1");
        history.push(":quit");
        history.push("");
        assert_eq!(history.entries, ["let x = 1"]);
    }

    #[test]
    fn the_history_is_capped() {
        let mut history = self::history(&[]);
        for i in 0..(MAX_HISTORY + 10) {
            history.push(&format!("line {i}"));
        }
        assert_eq!(history.entries.len(), MAX_HISTORY);
        let newest = format!("line {}", MAX_HISTORY + 9);
        assert_eq!(history.entries.last(), Some(&newest));
    }
}
