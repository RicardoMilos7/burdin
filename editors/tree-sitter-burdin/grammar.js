/**
 * @file The tree-sitter grammar for Burdin, for highlighting in editors (editors/README.md).
 *
 * It is lexical, like the VS Code TextMate grammar: the tokens of spec/grammar.md section 1 (comments, strings with
 * their `{...}` holes, numbers, keywords, operators) nested by their brackets, plus the few shapes highlighting needs
 * (a declared name after `fn`, `struct`, `enum`, `interface`, `type`, or `error`; a call; a member after `.`; a
 * `test "..."` item). It does not parse statements or expressions, so it accepts every Burdin file and never needs
 * changing for a new expression form; names are colored by what they resolve to through the language server's
 * semantic tokens. tests/check_lsp.py checks that KEYWORDS is exactly the lexer's list
 * (crates/burdin_syntax/src/token.rs).
 */

/* eslint-disable no-undef */

// The keywords of crates/burdin_syntax/src/token.rs, `derive` (reserved) included.
const KEYWORDS = [
  'any', 'as', 'break', 'comptime', 'const', 'continue', 'derive', 'else', 'enum', 'error', 'extern', 'false', 'fn',
  'for', 'if', 'impl', 'in', 'interface', 'let', 'loop', 'match', 'mut', 'orelse', 'own', 'priv', 'return', 'self',
  'Self', 'struct', 'true', 'type', 'unsafe', 'use', 'where', 'while',
];

// The keywords that start a type declaration and are followed by its name.
const TYPE_DECLARATIONS = ['struct', 'enum', 'interface', 'type', 'error'];

// Keywords that may be a member name after `.` (spec/grammar.md section 1: `.any(...)`).
const MEMBER_KEYWORDS = ['any', 'impl'];

// The operators of spec/grammar.md section 1 (Burdin has no compound assignment).
const OPERATORS = [
  '..', '...', '..=', '=>', '@', '?', '=', '+', '-', '*', '/', '%', '&', '|', '^', '<<', '>>', '!', '&&', '||', '==',
  '!=', '<', '>', '<=', '>=',
];

module.exports = grammar({
  name: 'burdin',

  extras: $ => [/\s/, $.line_comment, $.block_comment],

  word: $ => $.identifier,

  rules: {
    source_file: $ => repeat($._item),

    // Anything at all, in any order. A `:` is an item everywhere but at the top of an interpolation, where it starts
    // the format specifier.
    _item: $ => choice($._hole_item, ':'),

    _hole_item: $ => choice(
      $.function_declaration,
      $.type_declaration,
      $.test_declaration,
      $.call,
      $.member,
      $.block,
      $.parenthesized,
      $.bracketed,
      $.string,
      $.raw_string,
      $.char_literal,
      $.integer_literal,
      $.float_literal,
      $.boolean,
      $.self,
      $.self_type,
      $.identifier,
      ...KEYWORDS.filter(k => !['true', 'false', 'self', 'Self'].includes(k)),
      ...OPERATORS,
      ',',
      ';',
      '.',
    ),

    // `fn name` and the extension form `fn Type.name`.
    function_declaration: $ => prec.right(2, seq(
      'fn',
      optional(seq(field('receiver', $.identifier), '.')),
      field('name', $.identifier),
    )),

    type_declaration: $ => prec(2, seq(
      field('kind', choice(...TYPE_DECLARATIONS)),
      field('name', $.identifier),
    )),

    // `test` is contextual: a test item is the word `test` followed by its name (spec/test-runner.md).
    test_declaration: $ => prec(2, seq(
      field('keyword', alias('test', $.identifier)),
      field('name', $.string),
    )),

    call: $ => prec(1, seq(field('function', $.identifier), field('arguments', $.arguments))),

    // `.name` and `.name(...)`: a field, a method, an enum variant, or a module member.
    member: $ => prec.right(1, seq(
      '.',
      field('name', choice($.identifier, ...MEMBER_KEYWORDS.map(k => alias(k, $.identifier)))),
      optional(field('arguments', $.arguments)),
    )),

    arguments: $ => seq('(', repeat($._item), ')'),
    parenthesized: $ => seq('(', repeat($._item), ')'),
    bracketed: $ => seq('[', repeat($._item), ']'),
    block: $ => seq('{', repeat($._item), '}'),

    // Strings: escapes, `{{` and `}}`, and `{expr}` or `{expr:spec}` holes (spec/format.md).
    string: $ => seq(
      '"',
      repeat(choice($.string_content, $.escape_sequence, $.interpolation)),
      token.immediate('"'),
    ),

    string_content: _ => token.immediate(prec(1, /[^"\\{}]+/)),

    escape_sequence: _ => token.immediate(choice(/\\[nrt0\\"]/, '{{', '}}')),

    interpolation: $ => seq(
      token.immediate('{'),
      repeat($._hole_item),
      optional($.format_specifier),
      '}',
    ),

    format_specifier: _ => seq(':', optional(token.immediate(prec(1, /[^}"\n]+/)))),

    // `r"..."`, `r#"..."#`, and `r##"..."##`: no escapes and no holes.
    raw_string: _ => token(choice(
      /r"[^"]*"/,
      /r#"([^"]|"[^#])*"#/,
      /r##"([^"]|"[^#]|"#[^#])*"##/,
    )),

    char_literal: _ => token(seq("'", choice(/[^'\\\n]/, /\\[nrt0\\']/), "'")),

    integer_literal: _ => token(choice(/0x[0-9A-Fa-f_]+/, /0o[0-7_]+/, /0b[01_]+/, /[0-9][0-9_]*/)),

    float_literal: _ => token(/[0-9][0-9_]*(\.[0-9][0-9_]*([eE][+-]?[0-9_]+)?|[eE][+-]?[0-9_]+)/),

    boolean: _ => choice('true', 'false'),
    self: _ => 'self',
    self_type: _ => 'Self',

    identifier: _ => /[A-Za-z_][A-Za-z0-9_]*/,

    line_comment: _ => token(seq('//', /[^\n]*/)),
    block_comment: _ => token(/\/\*[^*]*\*+([^/*][^*]*\*+)*\//),
  },
});
