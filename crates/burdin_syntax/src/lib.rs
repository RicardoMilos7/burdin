//! Lexer, parser, and syntax tree for Burdin stage 0 (spec/grammar.md).

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

use burdin_diag::{Diagnostics, FileId};

/// Lexes and parses one source file. Errors go to `diags`; the returned tree holds whatever parsed.
pub fn parse(file: FileId, text: &str, diags: &mut Diagnostics) -> ast::File {
    let lexed = lexer::lex(file, text, diags);
    parser::parse_file(file, lexed, diags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::SourceMap;

    fn errors(src: &str) -> String {
        let mut sources = SourceMap::default();
        let id = sources.add("t.bd".into(), src.into());
        let mut d = Diagnostics::default();
        parse(id, src, &mut d);
        d.render(&sources)
    }

    #[test]
    fn parses_calls_and_comments() {
        let src = "/// doc\nfn say(text: str) {\n    println(text) // c\n}\nfn main() {\n    /* x */ say(\"a\")\n}\n";
        assert_eq!(errors(src), "");
    }

    #[test]
    fn expressions() {
        let src = "fn f(): i32 {\n let mut x = 1 + 2 * 3\n x = x -\n 1\n let y = x\n .abs()\n if a && b || c { 1 } else if d { 2 } else { 3 }\n while x < 10 { x = x + 1 }\n (x as i64, -x, !b, t.0.1)\n}\n";
        assert_eq!(errors(src), "");
    }

    #[test]
    fn reports_every_error() {
        let src = "fn main() {\n    let = 5\n    let x = )\n}\nfn g( {\n}\nfn h() {}\n";
        let out = errors(src);
        assert_eq!(out.lines().count(), 3, "{out}");
        assert!(out.contains("t.bd:2:9: error[E0001]"), "{out}");
        assert!(out.contains("t.bd:3:13: error[E0001]"), "{out}");
    }

    #[test]
    fn chained_comparison_is_an_error() {
        assert!(errors("fn f() {\n a < b < c\n}\n").contains("E0001"));
    }

    #[test]
    fn unterminated_string_reports_once() {
        let out = errors("fn main() {\n    println(\"never closed)\n}\n");
        assert_eq!(out, "t.bd:2:13: error[E0001]: unterminated string literal\n");
    }

    #[test]
    fn match_and_patterns() {
        let src = "fn f() {\n match x {\n  Some(v) if v > 0 => g(v),\n  None => {}\n  1 | 2 => {} 3..=9 => h()\n  -1 => {}, n @ 10..=20 => {}\n  Shape.Rect { w, h: 0 } => {}\n  else => {}\n }\n}\n";
        assert_eq!(errors(src), "");
    }

    #[test]
    fn const_items() {
        assert_eq!(errors("const A: i32 = 10\npriv const B: i32 = A * 2\nfn main() {}\n"), "");
    }

    #[test]
    fn exclusive_range_pattern_is_an_error() {
        assert!(errors("fn f() {\n match x {\n  1..5 => {}\n }\n}\n").contains("E0001"));
    }

    #[test]
    fn reserved_words_are_rejected() {
        assert!(errors("fn main() {\n let impl = 1\n}\n").contains("E0001"));
    }
}
