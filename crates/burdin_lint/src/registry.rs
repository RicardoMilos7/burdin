//! The lints, their codes and default levels, and the configuration that changes the levels (`spec/lint.md`, LN2-LN5).

use std::fmt;

/// How loudly a lint reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The lint does not run.
    Allow,
    /// The lint prints a warning; `burdin lint` still succeeds.
    Warn,
    /// The lint prints an error; `burdin lint` fails.
    Deny,
}

impl Level {
    pub fn parse(text: &str) -> Option<Level> {
        match text {
            "allow" => Some(Level::Allow),
            "warn" => Some(Level::Warn),
            "deny" => Some(Level::Deny),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Allow => "allow",
            Level::Warn => "warn",
            Level::Deny => "deny",
        }
    }
}

macro_rules! lints {
    ($( $variant:ident, $name:literal, $code:literal, $level:ident, $summary:literal; )*) => {
        /// A lint. Each has a name (what a configuration or a suppression comment writes), a code (what a
        /// diagnostic prints), a default level, and a one-line summary.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Lint {
            $( $variant, )*
        }

        impl Lint {
            /// Every lint, in code order.
            pub const ALL: &'static [Lint] = &[ $( Lint::$variant, )* ];

            pub fn name(self) -> &'static str {
                match self { $( Lint::$variant => $name, )* }
            }

            pub fn code(self) -> &'static str {
                match self { $( Lint::$variant => $code, )* }
            }

            pub fn default_level(self) -> Level {
                match self { $( Lint::$variant => Level::$level, )* }
            }

            pub fn summary(self) -> &'static str {
                match self { $( Lint::$variant => $summary, )* }
            }

            pub fn from_name(name: &str) -> Option<Lint> {
                match name { $( $name => Some(Lint::$variant), )* _ => None }
            }

            pub fn from_code(code: &str) -> Option<Lint> {
                match code { $( $code => Some(Lint::$variant), )* _ => None }
            }
        }
    };
}

lints! {
    NonSnakeCase, "non_snake_case", "L0001", Warn,
        "a function, variable, parameter, field, or module name is not snake_case";
    NonPascalCase, "non_pascal_case", "L0002", Warn,
        "a type, interface, enum variant, alias, or type parameter name is not PascalCase";
    NonScreamingCase, "non_screaming_case", "L0003", Warn,
        "a constant name is not SCREAMING_SNAKE_CASE";
    ElseOnLocalEnum, "else_on_local_enum", "L0004", Warn,
        "an `else` arm hides new variants of an enum declared in the same package";
    EagerChain, "eager_chain", "L0005", Warn,
        "a long chain of eager `List` methods builds a list at every step; start it with `.iter()`";
    UnusedImports, "unused_imports", "L0006", Warn,
        "a name imported with `use` is never used";
    UnusedVariables, "unused_variables", "L0007", Warn,
        "a local variable is never read";
    UnusedParameters, "unused_parameters", "L0008", Warn,
        "a function parameter is never read";
    UnreachableCode, "unreachable_code", "L0009", Warn,
        "a statement follows an expression that never completes";
    DeadCode, "dead_code", "L0010", Warn,
        "a `priv` item is never used in its package";
    RedundantClone, "redundant_clone", "L0011", Warn,
        "a `.clone()` whose original is never used again; move it instead";
    LargeCopyInLoop, "large_copy_in_loop", "L0012", Warn,
        "a large `Copy` value is passed by value inside a loop";
    UnusedMut, "unused_mut", "L0013", Warn,
        "a `mut` binding is never mutated";
    BorrowCouldOwn, "borrow_could_own", "L0014", Warn,
        "a borrowed parameter is cloned inside the function while every caller is done with the argument";
    LoopInvariantAlloc, "loop_invariant_alloc", "L0015", Warn,
        "an allocation inside a loop does not change between iterations; hoist it";
    NeedlessToList, "needless_to_list", "L0016", Warn,
        "`to_list()` builds a list only to iterate it right away";
}

impl fmt::Display for Lint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The level of every lint for one run: the defaults, then the manifest's `[lint]` table, then the command line.
#[derive(Clone, Debug)]
pub struct Config {
    levels: Vec<Level>,
}

impl Default for Config {
    fn default() -> Config {
        Config { levels: Lint::ALL.iter().map(|l| l.default_level()).collect() }
    }
}

impl Config {
    pub fn level(&self, lint: Lint) -> Level {
        self.levels[lint as usize]
    }

    /// Sets the level of the lint called `name`. `all` names every lint. `warnings` names the lints that are
    /// currently at `warn`, so `-D warnings` makes every warning an error without touching the allowed lints.
    pub fn set(&mut self, name: &str, level: Level) -> Result<(), String> {
        match name {
            "all" => self.levels.iter_mut().for_each(|l| *l = level),
            "warnings" => {
                for l in self.levels.iter_mut().filter(|l| **l == Level::Warn) {
                    *l = level;
                }
            }
            _ => match Lint::from_name(name).or_else(|| Lint::from_code(&name.to_ascii_uppercase())) {
                Some(lint) => self.levels[lint as usize] = level,
                None => return Err(unknown_lint(name)),
            },
        }
        Ok(())
    }

    /// Applies one `name = "level"` entry of a manifest's `[lint]` table.
    pub fn set_from_manifest(&mut self, name: &str, level: &str) -> Result<(), String> {
        let Some(parsed) = Level::parse(level) else {
            return Err(format!("`{level}` is not a lint level (expected allow, warn, or deny)"));
        };
        if name == "warnings" {
            return Err("`warnings` is a command-line group (`-D warnings`); name a lint".to_string());
        }
        self.set(name, parsed)
    }
}

fn unknown_lint(name: &str) -> String {
    let near = Lint::ALL.iter().find(|l| l.name().contains(name) || name.contains(l.name()));
    match near {
        Some(l) if !name.is_empty() => format!("unknown lint `{name}` (did you mean `{}`?)", l.name()),
        _ => format!("unknown lint `{name}` (`burdin lint --list` shows every lint)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_codes_are_unique_and_round_trip() {
        for (i, lint) in Lint::ALL.iter().enumerate() {
            assert_eq!(*lint as usize, i, "the variants are in code order");
            assert_eq!(Lint::from_name(lint.name()), Some(*lint));
            assert_eq!(Lint::from_code(lint.code()), Some(*lint));
            assert_eq!(lint.code(), format!("L{:04}", i + 1));
        }
    }

    #[test]
    fn levels_apply_in_order() {
        let mut config = Config::default();
        assert_eq!(config.level(Lint::DeadCode), Level::Warn);
        config.set("dead_code", Level::Allow).unwrap();
        config.set("warnings", Level::Deny).unwrap();
        assert_eq!(config.level(Lint::DeadCode), Level::Allow);
        assert_eq!(config.level(Lint::UnusedMut), Level::Deny);
        assert!(config.set("no_such_lint", Level::Deny).is_err());
        config.set("l0010", Level::Deny).unwrap();
        assert_eq!(config.level(Lint::DeadCode), Level::Deny, "a code names a lint too");
        config.set("all", Level::Allow).unwrap();
        assert_eq!(config.level(Lint::UnusedMut), Level::Allow);
    }

    #[test]
    fn manifest_entries_are_checked() {
        let mut config = Config::default();
        assert!(config.set_from_manifest("unused_mut", "deny").is_ok());
        assert!(config.set_from_manifest("unused_mut", "loud").is_err());
        assert!(config.set_from_manifest("warnings", "deny").is_err());
        assert!(config.set_from_manifest("unused", "deny").unwrap_err().contains("did you mean"));
    }
}
