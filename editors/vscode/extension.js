// The Burdin extension for VS Code (spec/lsp.md, LS20): it starts `burdin lsp`, passes the `burdin` settings, and
// implements the client commands of the lenses (LS19). Everything else is the language server's.
"use strict";

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

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("burdin.run", run),
    vscode.commands.registerCommand("burdin.debug", debug),
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
