---
title: Editor support
description: Set up VS Code, Zed, Neovim, or Helix with the Burdin language server.
---

Every editor talks to the same language server, `burdin lsp`. It reports errors as you type, shows the type of a
`let` without an annotation, finds definitions and references, formats code, and runs tests from a code lens. Any editor
with a Language Server Protocol client can use it. Make sure `burdin` is on your `PATH`, or point the editor at the
binary by its full path.

## Visual Studio Code

The repository has a VS Code extension in `editors/vscode`. It adds the Burdin language for `.bd` files, syntax
colors, the `Burdin: Run` and `Burdin: Debug` commands, and the test lenses. You install it from a checkout of the
repository:

```console
$ cd editors/vscode
$ bun install
$ bunx @vscode/vsce package
$ code --install-extension burdin-0.1.0.vsix
```

Open a folder that contains a `burdin.toml`. If `burdin` is not on your `PATH`, set `burdin.path` in the settings to its
location.

The extension's settings include `burdin.inlayHints.types` and `burdin.inlayHints.parameterNames`, which turn on the
inferred types after `let` names and the parameter names before positional arguments.

## Zed

The repository also has a Zed extension in `editors/zed`. Install it with **zed: install dev extension** from the command
palette and select that folder. Zed builds the extension to WebAssembly, so Rust must be installed through rustup. The
extension highlights code with a tree-sitter grammar and starts `burdin lsp` when you open a `.bd` file.

To use a particular `burdin` binary, set it in Zed's `settings.json`:

```json
{
  "lsp": {
    "burdin": {
      "binary": { "path": "/path/to/burdin" }
    }
  }
}
```

## Neovim

Neovim 0.11 and later can start the server with its built-in client:

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

Inlay hints are off by default in Neovim; `vim.lsp.inlay_hint.enable(true)` turns them on.

## Helix

Add this to `languages.toml` (Helix 24.03 and later):

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

Helix does not highlight Burdin code yet, but errors, completion, and navigation work.

## Formatting on save

The server formats code with the same rules as `burdin fmt`. Ask your editor to format on save, or run `burdin fmt .` before
you commit. Either way the result is the same, because there is only one style.

## If something does not work

- **No errors appear.** Check that `burdin lsp` starts from a terminal and that the folder has a `burdin.toml`, or is inside a
  Git repository.
- **The editor cannot find `burdin`.** Use the full path to the binary in the editor's settings.
- **Errors are out of date.** Save the file. The server checks a package a short time after each edit.
