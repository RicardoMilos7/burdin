//! Platform predicates: the `cfg(...)` of a `[target.'cfg(...)']` table (`spec/packages.md`, PKG27).
//!
//! The predicate is evaluated for the host the compiler runs on, which is the target it builds for. The grammar is
//! small on purpose: `unix`, `windows`, `target_os = "..."`, `target_arch = "..."`, `target_family = "..."`, and
//! `all(...)`, `any(...)`, `not(...)` over them. A name the compiler does not know is an error, so a typo does not
//! silently drop a dependency.

/// A parsed predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cfg {
    /// `unix`: Linux and macOS.
    Unix,
    /// `windows`.
    Windows,
    /// `target_os = "<os>"`.
    TargetOs(String),
    /// `target_arch = "<arch>"`.
    TargetArch(String),
    /// `target_family = "unix"` or `"windows"`.
    TargetFamily(String),
    /// `all(...)`: true when every predicate is (true for an empty list).
    All(Vec<Cfg>),
    /// `any(...)`: true when one predicate is (false for an empty list).
    Any(Vec<Cfg>),
    /// `not(...)`.
    Not(Box<Cfg>),
}

/// The platform a predicate is evaluated for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Host {
    /// `linux`, `macos`, or `windows` (`burdin_tir::target_os`).
    pub os: &'static str,
    /// `x86_64` or `aarch64` (`burdin_tir::target_arch`).
    pub arch: &'static str,
}

impl Host {
    /// The machine this compiler runs on.
    pub fn current() -> Host {
        Host { os: burdin_tir::target_os(), arch: burdin_tir::target_arch() }
    }

    fn is_windows(self) -> bool {
        self.os == "windows"
    }
}

impl Cfg {
    /// Whether the predicate holds on `host`.
    pub fn matches(&self, host: Host) -> bool {
        match self {
            Cfg::Unix => !host.is_windows(),
            Cfg::Windows => host.is_windows(),
            Cfg::TargetOs(os) => os == host.os,
            Cfg::TargetArch(arch) => arch == host.arch,
            Cfg::TargetFamily(family) => match family.as_str() {
                "unix" => !host.is_windows(),
                "windows" => host.is_windows(),
                _ => false,
            },
            Cfg::All(items) => items.iter().all(|item| item.matches(host)),
            Cfg::Any(items) => items.iter().any(|item| item.matches(host)),
            Cfg::Not(item) => !item.matches(host),
        }
    }
}

/// Parses the text of a `[target.'cfg(...)']` header's predicate: `cfg(` then one predicate then `)`. The error
/// is a message without its line; the manifest parser adds the line.
pub fn parse_cfg(text: &str) -> Result<Cfg, String> {
    let text = text.trim();
    let Some(inner) = text.strip_prefix("cfg(").and_then(|rest| rest.strip_suffix(')')) else {
        return Err(format!("a target table takes `cfg(...)`, found `{text}`"));
    };
    let mut parser = Parser { text: inner, at: 0 };
    let predicate = parser.predicate()?;
    parser.skip_space();
    if parser.at != parser.text.len() {
        return Err(format!("unexpected `{}` after the predicate", &parser.text[parser.at..]));
    }
    Ok(predicate)
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.rest().starts_with(' ') || self.rest().starts_with('\t') {
            self.at += self.rest().chars().next().map_or(1, char::len_utf8);
        }
    }

    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn eat(&mut self, token: char) -> bool {
        self.skip_space();
        if self.rest().starts_with(token) {
            self.at += token.len_utf8();
            true
        } else {
            false
        }
    }

    fn word(&mut self) -> Option<String> {
        self.skip_space();
        let len = self.rest().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if len == 0 {
            return None;
        }
        let word = self.rest()[..len].to_string();
        self.at += len;
        Some(word)
    }

    fn string(&mut self) -> Result<String, String> {
        self.skip_space();
        let Some(body) = self.rest().strip_prefix('"') else {
            return Err("expected a quoted value after `=`".to_string());
        };
        let Some(end) = body.find('"') else {
            return Err("an unterminated quoted value".to_string());
        };
        let value = body[..end].to_string();
        self.at += end + 2;
        Ok(value)
    }

    fn list(&mut self) -> Result<Vec<Cfg>, String> {
        let mut items = Vec::new();
        if self.eat(')') {
            return Ok(items);
        }
        loop {
            items.push(self.predicate()?);
            if self.eat(')') {
                return Ok(items);
            }
            if !self.eat(',') {
                return Err("expected `,` or `)` in the predicate list".to_string());
            }
            // A trailing comma is allowed, as in Rust.
            if self.eat(')') {
                return Ok(items);
            }
        }
    }

    fn predicate(&mut self) -> Result<Cfg, String> {
        let Some(name) = self.word() else {
            return Err("expected a predicate such as `unix`, `windows`, or `target_os = \"linux\"`".to_string());
        };
        match name.as_str() {
            "unix" => Ok(Cfg::Unix),
            "windows" => Ok(Cfg::Windows),
            "target_os" | "target_arch" | "target_family" => {
                if !self.eat('=') {
                    return Err(format!("`{name}` needs a value: `{name} = \"...\"`"));
                }
                let value = self.string()?;
                Ok(match name.as_str() {
                    "target_os" => Cfg::TargetOs(value),
                    "target_arch" => Cfg::TargetArch(value),
                    _ => Cfg::TargetFamily(value),
                })
            }
            "all" | "any" | "not" => {
                if !self.eat('(') {
                    return Err(format!("`{name}` takes a list in parentheses"));
                }
                let items = self.list()?;
                match name.as_str() {
                    "all" => Ok(Cfg::All(items)),
                    "any" => Ok(Cfg::Any(items)),
                    _ => match <[Cfg; 1]>::try_from(items) {
                        Ok([only]) => Ok(Cfg::Not(Box::new(only))),
                        Err(_) => Err("`not` takes exactly one predicate".to_string()),
                    },
                }
            }
            other => Err(format!(
                "unknown predicate `{other}`: this compiler knows unix, windows, target_os, target_arch, \
                 target_family, all, any, and not"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: Host = Host { os: "linux", arch: "x86_64" };
    const WINDOWS: Host = Host { os: "windows", arch: "x86_64" };
    const MAC: Host = Host { os: "macos", arch: "aarch64" };

    #[test]
    fn the_predicates_match_their_platforms() {
        assert_eq!(parse_cfg("cfg(unix)"), Ok(Cfg::Unix));
        assert!(Cfg::Unix.matches(LINUX) && Cfg::Unix.matches(MAC) && !Cfg::Unix.matches(WINDOWS));
        assert!(Cfg::Windows.matches(WINDOWS) && !Cfg::Windows.matches(LINUX));
        let os = parse_cfg("cfg(target_os = \"macos\")").unwrap();
        assert!(os.matches(MAC) && !os.matches(LINUX));
        let arch = parse_cfg("cfg(target_arch=\"aarch64\")").unwrap();
        assert!(arch.matches(MAC) && !arch.matches(LINUX));
        let family = parse_cfg("cfg(target_family = \"unix\")").unwrap();
        assert!(family.matches(LINUX) && !family.matches(WINDOWS));
        // A value no platform of this compiler has is a valid predicate that is false.
        assert!(!parse_cfg("cfg(target_os = \"freebsd\")").unwrap().matches(LINUX));
    }

    #[test]
    fn combinators_compose() {
        let both = parse_cfg("cfg(all(unix, not(target_os = \"macos\")))").unwrap();
        assert!(both.matches(LINUX) && !both.matches(MAC) && !both.matches(WINDOWS));
        let either = parse_cfg("cfg(any(windows, target_arch = \"aarch64\",))").unwrap();
        assert!(either.matches(WINDOWS) && either.matches(MAC) && !either.matches(LINUX));
        assert!(parse_cfg("cfg(all())").unwrap().matches(LINUX));
        assert!(!parse_cfg("cfg(any())").unwrap().matches(LINUX));
    }

    #[test]
    fn malformed_and_unknown_predicates_are_errors() {
        assert!(parse_cfg("unix").unwrap_err().contains("takes `cfg(...)`"));
        assert!(parse_cfg("cfg(wasm)").unwrap_err().contains("unknown predicate `wasm`"));
        assert!(parse_cfg("cfg(target_os)").unwrap_err().contains("needs a value"));
        assert!(parse_cfg("cfg(target_os = linux)").unwrap_err().contains("quoted value"));
        assert!(parse_cfg("cfg(not(unix, windows))").unwrap_err().contains("exactly one"));
        assert!(parse_cfg("cfg(all(unix)").unwrap_err().contains("expected `,` or `)`"));
        assert!(parse_cfg("cfg(unix windows)").unwrap_err().contains("unexpected"));
        assert!(parse_cfg("cfg()").unwrap_err().contains("expected a predicate"));
    }
}
