; Highlighting for Burdin. A later pattern wins over an earlier one for the same node, so the general cases come
; first. editors/zed/languages/burdin/highlights.scm is a copy of this file (tests/check_lsp.py checks that).

; Names. A capitalized name is a type, a variant, or an interface; an all-caps name is a constant.
(identifier) @variable

((identifier) @type
  (#match? @type "^[A-Z]"))

((identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z0-9_]+$"))

((identifier) @type.builtin
  (#any-of? @type.builtin
    "bool" "char" "str" "f32" "f64" "i8" "i16" "i32" "i64" "i128" "isize" "u8" "u16" "u32" "u64" "u128"
    "usize"))

((identifier) @variable.special
  (#eq? @variable.special "_"))

(self) @variable.special

(self_type) @type

; Members: fields, then methods, then variants (`Color.Red`) and module types (`fs.File`).
(member
  name: (identifier) @property)

(member
  name: (identifier) @function.method
  arguments: (arguments))

((member
  name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

; Calls and declarations.
(call
  function: (identifier) @function)

((call
  function: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

(function_declaration
  receiver: (identifier) @type)

(function_declaration
  name: (identifier) @function)

(type_declaration
  name: (identifier) @type)

(test_declaration
  keyword: (identifier) @keyword)

; Keywords.
[
  "break"
  "continue"
  "else"
  "for"
  "if"
  "in"
  "loop"
  "match"
  "orelse"
  "return"
  "while"
] @keyword

[
  "const"
  "enum"
  "error"
  "extern"
  "fn"
  "impl"
  "interface"
  "let"
  "struct"
  "type"
  "use"
] @keyword

[
  "any"
  "mut"
  "own"
  "priv"
  "unsafe"
  "where"
] @keyword

"as" @keyword

; Reserved for a later version: the compiler rejects it.
"derive" @keyword

; Literals.
(boolean) @boolean

[
  (integer_literal)
  (float_literal)
] @number

[
  (string)
  (raw_string)
  (char_literal)
] @string

(escape_sequence) @string.escape

(format_specifier) @string.special

; Comments; `///` and `//!` are documentation.
[
  (line_comment)
  (block_comment)
] @comment

((line_comment) @comment.doc
  (#match? @comment.doc "^//[/!]"))

; Operators and punctuation.
[
  ".."
  "..."
  "..="
  "=>"
  "@"
  "?"
  "="
  "+"
  "-"
  "*"
  "/"
  "%"
  "&"
  "|"
  "^"
  "<<"
  ">>"
  "!"
  "&&"
  "||"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
] @punctuation.delimiter

; The braces of a `{...}` hole in a string.
(interpolation
  [
    "{"
    "}"
  ] @punctuation.special)

; A format specifier is text, its `:` included.
(format_specifier
  ":" @string.special)
