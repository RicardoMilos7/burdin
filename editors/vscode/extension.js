// The Burdin extension for VS Code (spec/lsp.md, LS20): it starts `burdin lsp`, passes the `burdin` settings, and
// implements the client commands of the lenses (LS19) and the
// input prompts of the refactoring commands (LS18). Everything else is the language server's.
"use strict";

const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

function burdinPath() {
  const configured = vscode.workspace.getConfiguration("burdin").get("path");
  return configured && configured.length > 0 ? configured : "burdin";
}

// The `burdin` settings as one nested object, the shape the server reads (`inlayHints.types`, ...).
function settings() {
  return JSON.parse(JSON.stringify(vscode.workspace.getConfiguration("burdin")));
}

function start() {
  const command = burdinPath();
  const serverOptions = { command, args: ["lsp"] };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "burdin" }],
    initializationOptions: settings(),
    synchronize: { configurationSection: "burdin" },
    outputChannelName: "Burdin",
  };
  client = new LanguageClient("burdin", "Burdin", serverOptions, clientOptions);
  return client.start();
}

function quote(word) {
  return /^[A-Za-z0-9_./:\\=-]+$/.test(word) ? word : JSON.stringify(word);
}

// `burdin.run`: the lens's command line in a terminal, from the lens's working directory.
function run(argument) {
  if (!argument || !Array.isArray(argument.argv)) {
    vscode.window.showErrorMessage("Burdin: run this from a lens above `main`.");
    return;
  }
  const terminal = vscode.window.createTerminal({ name: "Burdin", cwd: argument.cwd });
  terminal.show();
  terminal.sendText(argument.argv.map(quote).join(" "));
}

// `burdin.debug`: the lens carries a DAP `launch` configuration of type `burdin` — a `target` (and a `test`) that
// `burdin dap` builds with `-g` before launching it (`spec/debug.md`, DBG11-DBG12). The descriptor factory below is
// the whole integration: the debugger is the driver's `dap` subcommand.
async function debug(launch) {
  if (!launch) {
    return;
  }
  const folder = vscode.workspace.workspaceFolders ? vscode.workspace.workspaceFolders[0] : undefined;
  if (!(await vscode.debug.startDebugging(folder, launch))) {
    vscode.window.showErrorMessage("Burdin: the debug session could not be started.");
  }
}

// The `burdin` debug type is `burdin dap`, which speaks DAP on standard input and output and forwards to lldb-dap
// (`spec/debug.md`, DBG11).
class BurdinDebugAdapterFactory {
  createDebugAdapterDescriptor() {
    return new vscode.DebugAdapterExecutable(burdinPath(), ["dap"]);
  }
}

// ---- refactoring commands (LS18): the server's `workspace/executeCommand` commands, with the input an editor asks for ----

function ask(options) {
  return vscode.window.showInputBox({ ignoreFocusOut: true, ...options });
}

// The server answers a workspace edit; the editor applies it. A refusal is the command's error, shown as is.
async function execute(command, argument) {
  if (!client) {
    vscode.window.showErrorMessage("Burdin: the language server is not running.");
    return;
  }
  try {
    const answer = await client.sendRequest("workspace/executeCommand", { command, arguments: [argument] });
    if (!answer) {
      return;
    }
    const edit = await client.protocolConverter.asWorkspaceEdit(answer);
    if (!(await vscode.workspace.applyEdit(edit))) {
      vscode.window.showErrorMessage("Burdin: the edit could not be applied.");
    }
  } catch (e) {
    vscode.window.showErrorMessage(`Burdin: ${e && e.message ? e.message : e}`);
  }
}

// The package directory of a file: the nearest folder above it with a `burdin.toml`.
function packageRoot(file) {
  let dir = path.dirname(file);
  for (;;) {
    if (fs.existsSync(path.join(dir, "burdin.toml"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      return undefined;
    }
    dir = parent;
  }
}

function activeBurdin() {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "burdin") {
    vscode.window.showErrorMessage("Burdin: open a Burdin file and put the cursor on the item first.");
    return undefined;
  }
  return editor;
}

function at(editor) {
  const p = editor.selection.active;
  return { uri: editor.document.uri.toString(), position: { line: p.line, character: p.character } };
}

// `burdin.searchReplace`: a structural pattern, then its replacement, over the package of the active file.
async function searchReplace() {
  const editor = activeBurdin();
  if (!editor) {
    return;
  }
  const root = packageRoot(editor.document.uri.fsPath);
  if (!root) {
    vscode.window.showErrorMessage("Burdin: the file is not in a package (no burdin.toml above it).");
    return;
  }
  const pattern = await ask({ title: "Search and replace: pattern", prompt: "A structural pattern, e.g. `$a.len() == 0`" });
  if (!pattern) {
    return;
  }
  const replacement = await ask({ title: "Search and replace: replacement", prompt: `Replace \`${pattern}\` with` });
  if (replacement === undefined) {
    return;
  }
  await execute("burdin.searchReplace", { pattern, replacement, root });
}

// One parameter as `name: Type` or `name: Type = default`, the shape the server's `add` takes.
function parseParameter(text) {
  const m = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^=]+?)\s*(?:=\s*(.+?))?\s*$/.exec(text);
  return m ? { name: m[1], type: m[2], default: m[3] } : undefined;
}

function names(text) {
  return text.split(",").map((n) => n.trim()).filter((n) => n.length > 0);
}

// `burdin.changeSignature`: choose the change, then its parameters, for the function at the cursor.
async function changeSignature() {
  const editor = activeBurdin();
  if (!editor) {
    return;
  }
  const kind = await vscode.window.showQuickPick(
    [
      { label: "Add a parameter", id: "add" },
      { label: "Remove parameters", id: "remove" },
      { label: "Reorder parameters", id: "reorder" },
      { label: "Rename parameters", id: "rename" },
    ],
    { title: "Change signature of the function at the cursor", ignoreFocusOut: true }
  );
  if (!kind) {
    return;
  }
  const argument = at(editor);
  if (kind.id === "add") {
    const text = await ask({
      title: "Add a parameter",
      prompt: "`name: Type`, or `name: Type = default` (a call written before the change passes the default)",
      value: "extra: i32 = 0",
      validateInput: (v) => (parseParameter(v) ? undefined : "Write `name: Type` or `name: Type = default`."),
    });
    if (text === undefined) {
      return;
    }
    argument.add = [parseParameter(text)];
  } else if (kind.id === "remove") {
    const text = await ask({ title: "Remove parameters", prompt: "Parameter names, separated by commas" });
    if (!text) {
      return;
    }
    argument.remove = names(text);
  } else if (kind.id === "reorder") {
    const text = await ask({ title: "Reorder parameters", prompt: "All parameter names in the new order, separated by commas" });
    if (!text) {
      return;
    }
    argument.reorder = names(text);
  } else {
    const text = await ask({
      title: "Rename parameters",
      prompt: "`old -> new`, separated by commas",
      validateInput: (v) =>
        names(v).every((p) => /^\w+\s*->\s*\w+$/.test(p)) ? undefined : "Write `old -> new`, separated by commas.",
    });
    if (!text) {
      return;
    }
    argument.rename = names(text).map((p) => {
      const [from, to] = p.split("->").map((x) => x.trim());
      return { from, to };
    });
  }
  await execute("burdin.changeSignature", argument);
}

// `burdin.moveItem`: pick the module the item at the cursor moves to (the package's `.bd` files), or type one.
async function moveItem() {
  const editor = activeBurdin();
  if (!editor) {
    return;
  }
  const root = packageRoot(editor.document.uri.fsPath);
  const found = root ? await vscode.workspace.findFiles(new vscode.RelativePattern(root, "**/*.bd"), "**/target/**") : [];
  const modules = found
    .map((f) => path.relative(root, f.fsPath).replace(/\\/g, "/").replace(/\.bd$/, "").replace(/\/mod$/, ""))
    .filter((m) => m !== "main" && m !== "lib")
    .map((m) => m.split("/").join("."))
    .sort();
  const picked =
    modules.length > 0
      ? await vscode.window.showQuickPick(modules, { title: "Move the item to the module", ignoreFocusOut: true })
      : await ask({ title: "Move the item to the module", prompt: "The module's name" });
  if (!picked) {
    return;
  }
  const argument = at(editor);
  argument.to = picked;
  await execute("burdin.moveItem", argument);
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("burdin.run", run),
    vscode.commands.registerCommand("burdin.debug", debug),
    vscode.commands.registerCommand("burdin.searchReplace", searchReplace),
    vscode.commands.registerCommand("burdin.changeSignature", changeSignature),
    vscode.commands.registerCommand("burdin.moveItem", moveItem),
    vscode.debug.registerDebugAdapterDescriptorFactory("burdin", new BurdinDebugAdapterFactory()),
    vscode.commands.registerCommand("burdin.restartServer", async () => {
      if (client) {
        await client.stop();
      }
      await start();
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      // A different executable needs a different server; the other settings reach the running one.
      if (e.affectsConfiguration("burdin.path") && client) {
        await client.stop();
        await start();
      }
    })
  );
  return start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
