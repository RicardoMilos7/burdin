# Editor support

Every editor talks to the same language server, `burdin lsp` ([`spec/lsp.md`](../spec/lsp.md)): it speaks the Language
Server Protocol over standard input and output, so any editor with an LSP client can use it. This directory has the VS
Code extension, the Zed extension, and the configuration for Neovim and Helix. `tests/check_lsp.py` checks the files
here (LS20); using them in an editor is the manual checklist in `spec/lsp.md`.

## Settings

The server reads these settings under `burdin`, as `initializationOptions` or through `workspace/didChangeConfiguration`
(LS5):

| Setting | Default | Meaning |
| --- | --- | --- |
| `inlayHints.types` | `true` | the inferred type after a `let` name without one |
| `inlayHints.parameterNames` | `true` | the parameter's name before a positional argument |
| `diagnostics.lints` | `true` | publish `burdin lint` diagnostics too |
| `diagnostics.debounceMs` | `200` | the quiet time after an edit before the package is checked again |
| `formatting.enabled` | `true` | answer formatting requests |
| `codeLens.enabled` | `true` | show the run and debug lenses |
| `path` | `""` | the `burdin` the test lenses run |

## VS Code

`vscode/` is the extension: the `burdin` language for `.bd` files, a TextMate grammar, the language configuration, the
settings above as `burdin.*`, and the commands the lenses use. To try it from this repository:

```
cd editors/vscode
npm install                      # vscode-languageclient, pinned in package.json
code --extensionDevelopmentPath="$PWD" /path/to/a/package
```

To install it, package it with `npx @vscode/vsce package` and install the `.vsix` with
`code --install-extension burdin-0.1.0.vsix`. `burdin` must be on `PATH`, or `burdin.path` must name it.

The `▶ Run test` lenses run in the server and show their result as a lens; `▶ Run` opens a terminal with the same
command line. `Debug` starts a session of type `burdin`: the extension registers that type against `burdin dap`
(`spec/debug.md`, DBG12), so the lenses, the Run and Debug view, and a `launch.json` configuration all use the
same adapter, and no debugger extension has to be installed. A `launch.json` entry names a `target` (a `.bd` file or
package directory, built with `-g` by the adapter) and optionally `test`:

```json
{
  "version": "0.2.0",
  "configurations": [
    { "type": "burdin", "request": "launch", "name": "Burdin", "target": "${workspaceFolder}" },
    { "type": "burdin", "request": "launch", "name": "Burdin test", "target": "${workspaceFolder}", "test": "loads" }
  ]
}
```

`burdin dap` needs `lldb-dap` (it ships with LLVM 21, whose `bin` directory is on `PATH` when `burdin` builds) and,
for a `target`, the same `burdin` that runs the adapter.

## Zed

`zed/` is the extension: the `Burdin` language for `.bd` files, highlighted by the tree-sitter grammar below (with
brackets, indentation, and the outline of functions, types, and tests), and the `burdin lsp` language server. Zed builds
its Rust part (`src/lib.rs`, against `zed_extension_api`, pinned in `Cargo.toml`) to WebAssembly, so Rust through rustup
must be installed. To install it from this repository, run `zed: install dev extension` from the command palette and
pick `editors/zed`.

`burdin` must be on `PATH`; the extension finds it there and starts `burdin lsp`. To use another `burdin`, or to change
the settings above, use the `lsp` table of Zed's `settings.json`:

```json
{
  "lsp": {
    "burdin": {
      "binary": { "path": "/path/to/burdin" },
      "settings": { "inlayHints": { "types": true, "parameterNames": false } }
    }
  }
}
```

`settings` is the object under `burdin`; it reaches the server as `initializationOptions` and again whenever it changes,
without a restart. A `binary.path` is also passed as the `path` setting, so the test lenses run the same `burdin`.
Inlay hints are off by default in Zed (`"inlay_hints": { "enabled": true }`). Code lenses are not shown in Zed; the
server's other features work.

Zed fetches the grammar from Git: `[grammars.burdin]` in `extension.toml` names this repository, the commit (`rev`), and
the grammar's directory. After changing the grammar or its `src/`, commit it and set `rev` to that commit.

## The tree-sitter grammar

`tree-sitter-burdin/` is the grammar Zed highlights with. Like the TextMate grammar, it is lexical: comments, strings
with their `{...}` holes and format specifiers, numbers, keywords, and operators, nested by their brackets, plus the
shapes highlighting needs (the name after `fn`, `struct`, `enum`, `interface`, `type`, and `error`; calls; members after
`.`; `test "..."`). It does not parse statements, so a new expression form needs no change; the language server's
semantic tokens color names by what they resolve to. `queries/highlights.scm` is the highlighting, and
`zed/languages/burdin/highlights.scm` is a copy of it.

`src/` is generated and committed, because editors compile `src/parser.c` themselves. After changing `grammar.js`:

```
cd editors/tree-sitter-burdin
npx tree-sitter-cli@0.26.13 generate --abi 14    # regenerates src/
npx tree-sitter-cli@0.26.13 test                 # test/corpus (trees) and test/highlight (highlight assertions)
```

then copy `queries/highlights.scm` to `zed/languages/burdin/`. `tests/check_lsp.py` checks that the grammar's keywords
are the lexer's, that `src/` is not older than them, that every node and field the queries name exists, and that the two
copies of `highlights.scm` agree.

## Neovim

Neovim 0.11 and later (`vim.lsp.config`):

```lua
vim.filetype.add({ extension = { bd = "burdin" } })

vim.lsp.config("burdin", {
  cmd = { "burdin", "lsp" },
  filetypes = { "burdin" },
  root_markers = { "burdin.toml", ".git" },
  settings = { burdin = { inlayHints = { types = true, parameterNames = true } } },
})
vim.lsp.enable("burdin")
```

Neovim 0.10: start the client from a `FileType` autocommand instead:

```lua
vim.filetype.add({ extension = { bd = "burdin" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "burdin",
  callback = function(args)
    vim.lsp.start({
      name = "burdin",
      cmd = { "burdin", "lsp" },
      root_dir = vim.fs.root(args.buf, { "burdin.toml", ".git" }),
    })
  end,
})
```

Inlay hints are off by default in Neovim; `vim.lsp.inlay_hint.enable(true)` turns them on. The lens commands
`burdin.runTest` and `burdin.runTests` run in the server (`vim.lsp.codelens.refresh()` shows the lenses and
`vim.lsp.codelens.run()` runs the one under the cursor); `burdin.run` and `burdin.debug` are editor commands that
Neovim does not have.

To debug from Neovim, point the built-in DAP client at `burdin dap` (`spec/debug.md`, DBG11-DBG12). A launch
configuration names the package as `target`, and `test` selects one test block:

```lua
require("dap").configurations.burdin = {
  {
    type = "executable",
    command = "burdin",
    args = { "dap" },
    name = "Burdin",
    request = "launch",
    target = vim.fn.getcwd(),
  },
  {
    type = "executable",
    command = "burdin",
    args = { "dap" },
    name = "Burdin (test)",
    request = "launch",
    target = vim.fn.getcwd(),
    test = "loads",
  },
}
```

`burdin dap` builds the `target` with `-g`, imports its lldb formatters, and breaks on `burdin_panic_begin` when the
session's "Burdin panic" exception filter is on.

## Helix

In `languages.toml` (Helix 24.03 and later):

```toml
[language-server.burdin]
command = "burdin"
args = ["lsp"]

[language-server.burdin.config.burdin]
inlayHints = { types = true, parameterNames = true }

[[language]]
name = "burdin"
scope = "source.burdin"
file-types = ["bd"]
roots = ["burdin.toml"]
comment-token = "//"
block-comment-tokens = { start = "/*", end = "*/" }
indent = { tab-width = 4, unit = "    " }
language-servers = ["burdin"]
```

Helix highlights with tree-sitter grammars; `tree-sitter-burdin` is not set up for Helix yet (its query captures follow
Zed's), so files are not highlighted; the
server's other features work. Code lenses are not shown in Helix.

Helix debugs through its DAP client and a debugger adapter command, `burdin dap` (`spec/debug.md`, DBG12). In
`languages.toml`, next to the language above:

```toml
[language.debugger]
name = "burdin"
transport = "stdio"
command = "burdin"
args = ["dap"]
```

and in `languages.toml`'s `[[language]]` entry for `burdin`, name it: `debugger = { name = "burdin", transport =
"stdio" }`. A launch template (`:debug-remote`/`b` in Helix's debug prompt) then names the package:

```toml
[[language.debugger.templates]]
name = "launch"
request = "launch"
completion = [{ name = "target", completion = "filename" }]
args = { target = "{0}" }
```

`test = "<name>"` in `args` debugs one test block instead of `main`.
