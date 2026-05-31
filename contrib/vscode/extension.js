const { LanguageClient } = require("vscode-languageclient/node");
const vscode = require("vscode");
const path = require("path");
const fs = require("fs");

let client;

function findServerBinary(context) {
  // 1. User-configured path takes priority
  const configured = vscode.workspace.getConfiguration("kmpLsp").get("path", "");
  if (configured) return configured;

  // 2. Bundled binary in platform-specific .vsix
  //    On Windows the binary is packaged as kmp-lsp.exe; on Unix it has no extension.
  const ext = process.platform === "win32" ? ".exe" : "";
  const bundled = path.join(context.extensionPath, "server", `kmp-lsp${ext}`);
  if (fs.existsSync(bundled)) return bundled;

  // 3. Fall back to PATH
  return "kmp-lsp";
}

function activate(context) {
  const command = findServerBinary(context);

  const serverOptions = { command };

  const clientOptions = {
    documentSelector: [
      { scheme: "file", language: "kotlin" },
      { scheme: "file", language: "java" },
      { scheme: "file", language: "swift" },
    ],
  };

  client = new LanguageClient("kmp-lsp", "Kotlin LSP", serverOptions, clientOptions);
  client.start();
}

function deactivate() {
  return client?.stop();
}

module.exports = { activate, deactivate };
