// Active language client (C-057): dependency-free Node entry point
// (vscode-languageclient ships in node_modules). Spawns `active-lsp`
// over stdio; the server path comes from settings so users without
// PATH installs keep working.
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client = null;

function serverOptions(context) {
	const configured = vscode.workspace
		.getConfiguration("active")
		.get("languageServer.path", "active-lsp");
	return {
		command: configured,
		args: [],
		transport: TransportKind.stdio,
		options: { cwd: context.extensionPath },
	};
}

function clientOptions() {
	return {
		documentSelector: [{ scheme: "file", language: "active" }],
	};
}

function activate(context) {
	// Object form (not a function): the function form of
	// ServerOptions must return a Promise, and a sync Executable
	// breaks server spawn (vscode-languageclient 9.0.1 typings).
	client = new LanguageClient(
		"active",
		"active",
		serverOptions(context),
		clientOptions(),
	);
	client.start();
	context.subscriptions.push(
		vscode.workspace.onDidChangeConfiguration((event) => {
			if (event.affectsConfiguration("active.languageServer")) {
				vscode.window.showInformationMessage(
					"Active server path changed — reload the window to restart active-lsp.",
				);
			}
		}),
	);
}

function deactivate() {
	return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
