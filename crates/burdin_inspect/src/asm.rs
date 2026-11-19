//! `burdin asm` (`spec/inspect.md`, INS9-INS11): one function's assembly or LLVM IR cut out of the whole program's,
//! with the Burdin source lines interleaved.
//!
//! The assembly is the text LLVM printed for a build with debug information, so every instruction run is preceded by a
//! `.loc <file> <line> <column>` directive and every file is introduced by `.file <id> "<dir>" "<name>"`. The view
//! drops the directives that only the assembler reads, and turns each change of source line into a comment that
//! carries the source text.

use std::collections::HashMap;

/// The lines of `symbol`'s definition, from its label to the end of the function (`.cfi_endproc` on ELF and Mach-O,
/// `.seh_endproc` on COFF). `None` when the program has no such label.
pub fn function_asm<'a>(asm: &'a str, symbol: &str) -> Option<Vec<&'a str>> {
    let mut labels = Vec::new();
    // Mach-O prefixes every C-level symbol with an underscore, and LLVM quotes a name with characters an assembler
    // would not read as one word (`<`, `,`, a space).
    for prefix in ["", "_"] {
        labels.push(format!("{prefix}{symbol}:"));
        labels.push(format!("\"{prefix}{symbol}\":"));
    }
    let lines: Vec<&str> = asm.lines().collect();
    let start = lines.iter().position(|line| labels.iter().any(|label| line.starts_with(label.as_str())))?;
    let mut end = lines.len();
    for (offset, line) in lines[start..].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == ".cfi_endproc" || trimmed == ".seh_endproc" {
            end = start + offset + 1;
            break;
        }
    }
    Some(lines[start..end].to_vec())
}

/// The `.file` table of an assembly text: file id to path, with `/` as the separator.
pub fn file_table(asm: &str) -> HashMap<u32, String> {
    let mut files = HashMap::new();
    for line in asm.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(".file") else { continue };
        let rest = rest.trim_start();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(id) = digits.parse::<u32>() else { continue };
        let strings = quoted_strings(&rest[digits.len()..]);
        let path = match strings.as_slice() {
            [] => continue,
            [path] => path.clone(),
            [dir, name, ..] => {
                if name.starts_with('/') || name.contains(':') {
                    name.clone()
                } else {
                    format!("{dir}/{name}")
                }
            }
        };
        files.insert(id, path.replace('\\', "/"));
    }
    files
}

/// The double-quoted strings in `text`, with `\\` and `\"` unescaped.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => value.extend(chars.next()),
                _ => value.push(c),
            }
        }
        out.push(value);
    }
    out
}

/// A `.loc <file> <line> <column> ...` directive, as `(file, line)`.
fn parse_loc(trimmed: &str) -> Option<(u32, u32)> {
    let rest = trimmed.strip_prefix(".loc")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut words = rest.split_whitespace();
    let file = words.next()?.parse().ok()?;
    let line = words.next()?.parse().ok()?;
    Some((file, line))
}

/// Whether a line is a directive or label that only the assembler or the unwinder reads.
fn is_noise(trimmed: &str) -> bool {
    const DIRECTIVES: [&str; 16] = [
        ".cfi_",
        ".p2align",
        ".type",
        ".size",
        ".globl",
        ".weak",
        ".hidden",
        ".section",
        ".text",
        ".seh_",
        ".def",
        ".scl",
        ".endef",
        ".file",
        ".addrsig",
        ".prologue_end",
    ];
    if DIRECTIVES.iter().any(|d| trimmed.starts_with(d)) {
        return true;
    }
    // Labels LLVM makes for its own bookkeeping; the `.LBB` labels are jump targets and stay.
    ["Ltmp", "Lfunc_begin", "Lfunc_end"]
        .iter()
        .any(|prefix| trimmed.strip_prefix('.').and_then(|t| t.strip_prefix(prefix)).is_some_and(|t| t.contains(':')))
}

/// `symbol`'s assembly with the source interleaved. `source_text` gives the lines of a file by the path the `.file`
/// table names; a file it does not know gets its `path:line` only. `comment` starts a comment line (`#` on x86).
pub fn render_asm(
    asm: &str,
    symbol: &str,
    source_text: &dyn Fn(&str) -> Option<String>,
    comment: &str,
) -> Option<String> {
    let body = function_asm(asm, symbol)?;
    let files = file_table(asm);
    let mut sources: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut out = String::new();
    let mut last: Option<(u32, u32)> = None;
    for line in body {
        let trimmed = line.trim_start();
        if let Some((file, number)) = parse_loc(trimmed) {
            if number == 0 || last == Some((file, number)) {
                continue;
            }
            last = Some((file, number));
            let Some(path) = files.get(&file) else { continue };
            let text = sources
                .entry(path.clone())
                .or_insert_with(|| source_text(path).map(|t| t.lines().map(str::to_string).collect()));
            let shown = text.as_ref().and_then(|lines| lines.get(number as usize - 1)).map(|l| l.trim());
            if !out.is_empty() && !out.ends_with("\n\n") && !out.ends_with(":\n") {
                out.push('\n');
            }
            match shown {
                Some(code) => out.push_str(&format!("    {comment} {path}:{number}: {code}\n")),
                None => out.push_str(&format!("    {comment} {path}:{number}\n")),
            }
            continue;
        }
        if is_noise(trimmed) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// `symbol`'s definition in LLVM IR text, from its `define` line to the closing brace.
pub fn function_ir(ir: &str, symbol: &str) -> Option<String> {
    let needles = [format!("@{symbol}("), format!("@\"{symbol}\"(")];
    let lines: Vec<&str> = ir.lines().collect();
    let start =
        lines.iter().position(|l| l.starts_with("define ") && needles.iter().any(|n| l.contains(n.as_str())))?;
    let end = lines[start..].iter().position(|l| *l == "}")?;
    let mut out = lines[start..=start + end].join("\n");
    out.push('\n');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASM: &str = "\t.file\t\"burdin\"\n\t.file\t1 \"/work/src\" \"main.bd\" md5 0xabc\n\t.text\n\t.globl\t\"burdin.main.largest<i32>\"\n\t.type\t\"burdin.main.largest<i32>\",@function\n\"burdin.main.largest<i32>\":                # @\"burdin.main.largest<i32>\"\n\t.cfi_startproc\n.Lfunc_begin0:\n\t.loc\t1 3 1\n\tmov\teax, edi\n\t.loc\t1 4 8 prologue_end\n\tcmp\tedi, esi\n\t.loc\t1 4 8\n\tjle\t.LBB0_2\n.LBB0_2:\n\t.loc\t1 5 1\n\tret\n.Ltmp0:\n.Lfunc_end0:\n\t.size\t\"burdin.main.largest<i32>\", .Lfunc_end0-\"burdin.main.largest<i32>\"\n\t.cfi_endproc\n\t.globl\tburdin.main\nburdin.main:\n\tret\n\t.cfi_endproc\n";

    #[test]
    fn a_function_is_cut_from_its_label_to_the_end_of_the_procedure() {
        let lines = function_asm(ASM, "burdin.main.largest<i32>").unwrap();
        assert!(lines[0].starts_with("\"burdin.main.largest<i32>\":"));
        assert_eq!(lines.last().unwrap().trim(), ".cfi_endproc");
        assert!(!lines.iter().any(|l| l.contains("burdin.main:")));
        // A name that is a prefix of another is not confused with it.
        let main = function_asm(ASM, "burdin.main").unwrap();
        assert_eq!(main.len(), 3, "{main:?}");
        assert!(function_asm(ASM, "burdin.missing").is_none());
    }

    #[test]
    fn the_file_table_joins_the_directory_and_the_name() {
        let files = file_table(ASM);
        assert_eq!(files.get(&1).map(String::as_str), Some("/work/src/main.bd"));
        assert_eq!(files.len(), 1, "the header `.file` has no id");
    }

    #[test]
    fn source_lines_are_interleaved_once_per_change_of_line() {
        let source = |path: &str| {
            assert_eq!(path, "/work/src/main.bd");
            Some("fn x() {}\n\nfn largest(a: i32, b: i32): i32 {\n    if a > b { a } else { b }\n}\n".to_string())
        };
        let text = render_asm(ASM, "burdin.main.largest<i32>", &source, "#").unwrap();
        assert_eq!(text.matches("# /work/src/main.bd:4:").count(), 1, "{text}");
        assert!(text.contains("# /work/src/main.bd:3: fn largest(a: i32, b: i32): i32 {\n"), "{text}");
        assert!(text.contains("# /work/src/main.bd:4: if a > b { a } else { b }\n"), "{text}");
        assert!(text.contains("\tcmp\tedi, esi\n") && text.contains(".LBB0_2:\n"), "{text}");
        for noise in [".cfi_", ".loc", ".Ltmp", ".Lfunc", ".size", ".type"] {
            assert!(!text.contains(noise), "`{noise}` should be dropped:\n{text}");
        }
    }

    #[test]
    fn a_file_the_lookup_does_not_know_still_names_its_line() {
        let text = render_asm(ASM, "burdin.main.largest<i32>", &|_| None, "//").unwrap();
        assert!(text.contains("    // /work/src/main.bd:5\n"), "{text}");
    }

    #[test]
    fn ir_is_cut_from_define_to_the_closing_brace() {
        let ir = "declare void @f()\n\ndefine i32 @\"burdin.main.largest<i32>\"(i32 %0, i32 %1) {\nentry:\n  ret i32 %0\n}\n\ndefine void @burdin.main() {\n  ret void\n}\n";
        let text = function_ir(ir, "burdin.main.largest<i32>").unwrap();
        assert!(text.starts_with("define i32 @\"burdin.main.largest<i32>\"") && text.ends_with("}\n"));
        assert!(!text.contains("burdin.main()"));
        assert!(function_ir(ir, "burdin.main").unwrap().contains("ret void"));
        assert!(function_ir(ir, "nope").is_none());
    }
}
