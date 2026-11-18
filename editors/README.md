# Editor support

Every editor talks to the same language server, `burdin lsp` ([`spec/lsp.md`](../spec/lsp.md)): it speaks the Language
Server Protocol over standard input and output, so any editor with an LSP client can use it. This directory has the VS
Code extension and the configuration for Neovim and Helix. `tests/check_lsp.py` checks the files here (LS20); using them
in an editor is the manual checklist in `spec/lsp.md`.

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
command line. `Debug` starts a session of type `burdin` once `burdin dap` exists (issue #576); until then a program is
built with `-g` and started under [CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb)
when it is installed.

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

Helix highlights with tree-sitter grammars, and there is none for Burdin yet, so files are not highlighted; the
server's other features work. Code lenses are not shown in Helix.
