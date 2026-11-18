// The Burdin extension for VS Code (spec/lsp.md, LS20): it starts `burdin lsp`, passes the `burdin` settings, and
// implements the client commands of the lenses (LS19). Everything else is the language server's.
"use strict";

const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");
const os = require("os");
const path = require("path");

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

// `burdin.debug`: the lens carries a DAP `launch` configuration of type `burdin`, for `burdin dap` (issue #576). Until
// a debug adapter of that type is installed, a program is built with `-g` and started under CodeLLDB when it is
// installed; a test has no fallback.
async function debug(launch) {
  if (!launch) {
    return;
  }
  const folder = vscode.workspace.workspaceFolders ? vscode.workspace.workspaceFolders[0] : undefined;
  try {
    if (await vscode.debug.startDebugging(folder, launch)) {
      return;
    }
  } catch (_) {
    // No adapter for `burdin`: fall through.
  }
  if (launch.test) {
    vscode.window.showInformationMessage("Burdin: debugging a test needs `burdin dap` (issue #576).");
    return;
  }
  if (!vscode.extensions.getExtension("vadimcn.vscode-lldb")) {
    vscode.window.showInformationMessage("Burdin: install CodeLLDB to debug a program until `burdin dap` exists.");
    return;
  }
  const output = path.join(os.tmpdir(), `burdin-debug-${process.pid}${process.platform === "win32" ? ".exe" : ""}`);
  const build = new vscode.ProcessExecution(burdinPath(), ["build", launch.program, "-g", "-o", output], {
    cwd: launch.cwd,
  });
  const task = new vscode.Task({ type: "process" }, vscode.TaskScope.Workspace, "build for debugging", "burdin", build);
  const execution = await vscode.tasks.executeTask(task);
  const code = await new Promise((resolve) => {
    const done = vscode.tasks.onDidEndTaskProcess((e) => {
      if (e.execution === execution) {
        done.dispose();
        resolve(e.exitCode);
      }
    });
  });
  if (code !== 0) {
    vscode.window.showErrorMessage("Burdin: the build for debugging failed.");
    return;
  }
  await vscode.debug.startDebugging(folder, {
    type: "lldb",
    request: "launch",
    name: launch.name,
    program: output,
    cwd: launch.cwd,
  });
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("burdin.run", run),
    vscode.commands.registerCommand("burdin.debug", debug),
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
