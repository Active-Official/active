// Active language client + tooling UX (TOOLING §9, C-057).
//
// The LSP client is the core: it spawns `active-lsp` over stdio.
// The tooling layer adds a status bar (mode, budget headroom, taint),
// commands for inspect/graph/repair/capability/check, and a read-only
// webview for the repair browser. Every CLI call is read-only except
// `repair --browser` (also read-only: it prints a plan).
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");
const { execFile } = require("child_process");

let client = null;
let status = null;
let output = null;
let refreshTimer = null;

function cliPath() {
	return vscode.workspace.getConfiguration("active").get("cli.path", "active");
}

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

/// Run the Active CLI and resolve stdout (rejects with stderr).
function run(args, cwd) {
	return new Promise((resolve, reject) => {
		execFile(
			cliPath(),
			args,
			{ cwd, timeout: 30000, maxBuffer: 8 << 20 },
			(err, stdout, stderr) => {
				if (err) {
					reject(new Error(String(stderr || err.message).slice(0, 800)));
					return;
				}
				resolve(String(stdout));
			},
		);
	});
}

function activeFile() {
	const editor = vscode.window.activeTextEditor;
	if (!editor || editor.document.languageId !== "active") return null;
	if (editor.document.uri.scheme !== "file") return null;
	return editor.document;
}

function json(text) {
	try {
		return JSON.parse(text);
	} catch {
		return null;
	}
}

/// Refresh the status bar: mode + budget headroom + taint presence.
/// Runs on activation, save, and editor switch; debounced.
async function refreshStatus() {
	if (!status) return;
	const document = activeFile();
	if (!document) {
		status.hide();
		return;
	}
	const file = document.uri.fsPath;
	const cwd = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath;
	try {
		const [modeText, budgetText, taintText] = await Promise.all([
			run(["mode", "--from", file, "--format", "json"], cwd),
			run(["viz", "budget", "--from", file], cwd),
			run(["viz", "taint", "--from", file], cwd),
		]);
		const mode = json(modeText);
		const budget = json(budgetText);
		const taint = json(taintText);
		const nodes = budget?.usage?.nodes ?? 0;
		const limit = budget?.limits?.nodes ?? 0;
		const pct = limit ? Math.round((nodes / limit) * 100) : 0;
		const secrets = (taint?.sources ?? []).length;
		status.text = `$(symbol-misc) Active ${mode?.mode ?? "?"} · budget ${pct}%${
			secrets ? " · taint" : ""
		}`;
		status.tooltip = `mode: ${mode?.mode ?? "?"}\ncapabilities: ${
			(mode?.capabilities ?? []).join(", ") || "(none)"
		}\nnodes: ${nodes}/${limit}\nsecret sources: ${secrets}`;
		status.show();
	} catch (e) {
		status.text = "$(warning) Active CLI unavailable";
		status.tooltip = String(e.message);
		status.show();
	}
}

function scheduleRefresh() {
	if (refreshTimer) clearTimeout(refreshTimer);
	refreshTimer = setTimeout(() => refreshStatus(), 250);
}

function show(title, body) {
	if (!output) output = vscode.window.createOutputChannel("Active");
	output.clear();
	output.appendLine(`# ${title}`);
	output.appendLine(body);
	output.show(true);
}

function showWebview(title, body) {
	const panel = vscode.window.createWebviewPanel(
		"active.panel",
		title,
		vscode.ViewColumn.Beside,
		{ enableScripts: false },
	);
	const escaped = String(body)
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;");
	panel.webview.html = `<!DOCTYPE html><html><head><meta charset="utf-8">
		<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
		<style>body{font-family:var(--vscode-font-family);padding:1em}
		pre{white-space:pre-wrap;word-break:break-word}</style></head>
		<body><h2>${escaped.split("\n")[0]}</h2><pre>${escaped}</pre></body></html>`;
}

function registerTooling(context) {
	status = vscode.window.createStatusBarItem(
		vscode.StatusBarAlignment.Right,
		100,
	);
	status.command = "active.refreshStatus";
	context.subscriptions.push(status);

	context.subscriptions.push(
		vscode.commands.registerCommand("active.refreshStatus", refreshStatus),
		vscode.commands.registerCommand("active.check", async () => {
			const document = activeFile();
			if (!document) return;
			try {
				const text = await run(["check", document.uri.fsPath]);
				show("active check", text || "OK");
			} catch (e) {
				show("active check", String(e.message));
			}
		}),
		vscode.commands.registerCommand("active.graph", async () => {
			const document = activeFile();
			if (!document) return;
			try {
				const text = await run([
					"graph",
					document.uri.fsPath,
					"--format",
					"mermaid",
				]);
				show("active graph (mermaid)", text);
			} catch (e) {
				show("active graph (mermaid)", String(e.message));
			}
		}),
		vscode.commands.registerCommand("active.inspect", async () => {
			const document = activeFile();
			if (!document) return;
			const path = await vscode.window.showInputBox({
				title: "Inspect path",
				prompt: "Dotted value path (for example server.port)",
			});
			if (!path) return;
			try {
				const text = await run([
					"inspect",
					"--from",
					document.uri.fsPath,
					"--path",
					path,
					"--format",
					"text",
				]);
				show(`inspect ${path}`, text);
			} catch (e) {
				show(`inspect ${path}`, String(e.message));
			}
		}),
		vscode.commands.registerCommand("active.repair", async () => {
			const document = activeFile();
			if (!document) return;
			try {
				const text = await run([
					"repair",
					"--from",
					document.uri.fsPath,
					"--browser",
				]);
				showWebview("Active repair plan", text);
			} catch (e) {
				show("Active repair plan", String(e.message));
			}
		}),
		vscode.commands.registerCommand("active.capability", async () => {
			try {
				const text = await run(["capability"]);
				showWebview("Active capability descriptor", text);
			} catch (e) {
				show("Active capability descriptor", String(e.message));
			}
		}),
		vscode.workspace.onDidSaveTextDocument((document) => {
			if (document.languageId === "active") scheduleRefresh();
		}),
		vscode.window.onDidChangeActiveTextEditor(() => scheduleRefresh()),
	);

	scheduleRefresh();
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
	registerTooling(context);
	context.subscriptions.push(
		vscode.workspace.onDidChangeConfiguration((event) => {
			if (event.affectsConfiguration("active.languageServer")) {
				vscode.window.showInformationMessage(
					"Active server path changed — reload the window to restart active-lsp.",
				);
			}
			if (event.affectsConfiguration("active.cli")) scheduleRefresh();
		}),
	);
}

function deactivate() {
	return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
