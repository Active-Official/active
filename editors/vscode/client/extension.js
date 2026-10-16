// SOLID language client (C-057): dependency-free Node entry point
// (vscode-languageclient ships in node_modules). Spawns `solid-ls`
// over stdio; the server path comes from settings so users without
// PATH installs keep working.
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client = null;

function serverOptions(context) {
	const configured = vscode.workspace
		.getConfiguration("solid")
		.get("languageServer.path", "solid-ls");
	return {
		command: configured,
		args: [],
		transport: TransportKind.stdio,
		options: { cwd: context.extensionPath },
	};
}

function clientOptions() {
	return {
		documentSelector: [{ scheme: "file", language: "solid" }],
	};
}

function activate(context) {
	// Object form (not a function): the function form of
	// ServerOptions must return a Promise, and a sync Executable
	// breaks server spawn (vscode-languageclient 9.0.1 typings).
	client = new LanguageClient(
		"solid",
		"SOLID",
		serverOptions(context),
		clientOptions(),
	);
	client.start();
	context.subscriptions.push(
		vscode.workspace.onDidChangeConfiguration((event) => {
			if (event.affectsConfiguration("solid.languageServer")) {
				vscode.window.showInformationMessage(
					"SOLID server path changed — reload the window to restart solid-ls.",
				);
			}
		}),
	);
}

function deactivate() {
	return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
