// Editor smoke test (TOOLING EDITOR.006): activates the real extension
// against a stubbed VS Code API and asserts the contribution surface.
// No VS Code install and no node_modules needed; the language-client
// module is stubbed as well.
const assert = require("assert");
const path = require("path");
const Module = require("module");
const { execFileSync } = require("child_process");

const registered = [];
let statusCreated = 0;
let webviews = 0;

const statusItem = {
	text: "",
	tooltip: "",
	command: "",
	show() {},
	hide() {},
};

const vscodeStub = {
	StatusBarAlignment: { Right: 2 },
	ViewColumn: { Beside: 2 },
	window: {
		createStatusBarItem() {
			statusCreated += 1;
			return statusItem;
		},
		createOutputChannel() {
			return { clear() {}, appendLine() {}, show() {} };
		},
		createWebviewPanel() {
			webviews += 1;
			return { webview: { html: "" } };
		},
		onDidChangeActiveTextEditor() {
			return { dispose() {} };
		},
		activeTextEditor: undefined,
		async showInputBox() {
			return undefined;
		},
		showInformationMessage() {},
	},
	workspace: {
		getConfiguration() {
			return { get: (_key, fallback) => fallback };
		},
		getWorkspaceFolder() {
			return undefined;
		},
		onDidSaveTextDocument() {
			return { dispose() {} };
		},
		onDidChangeConfiguration() {
			return { dispose() {} };
		},
	},
	commands: {
		registerCommand(name) {
			registered.push(name);
			return { dispose() {} };
		},
	},
};

class LanguageClientStub {
	constructor() {}
	start() {}
	stop() {
		return Promise.resolve();
	}
}

const originalLoad = Module._load;
Module._load = function (request, parent, isMain) {
	if (request === "vscode") return vscodeStub;
	if (request === "vscode-languageclient/node") {
		return { LanguageClient: LanguageClientStub, TransportKind: { stdio: 1 } };
	}
	return originalLoad.call(this, request, parent, isMain);
};

const extension = require(path.join(__dirname, "..", "client", "extension.js"));
Module._load = originalLoad;

(async () => {
	// Put the built CLI on PATH so the status refresh drives the real
	// success path; a fake active editor makes the document resolvable.
	const fs = require("fs");
	const os = require("os");
	const fixture = path.join(os.tmpdir(), `active-editor-${process.pid}.active`);
	fs.writeFileSync(fixture, 'token = secret("hunter2-editor")\nport = 8080\n');
	const binDir = path.join(__dirname, "..", "..", "..", "target", "debug");
	if (fs.existsSync(binDir)) {
		process.env.PATH = `${binDir}${path.delimiter}${process.env.PATH ?? ""}`;
	}
	vscodeStub.window.activeTextEditor = {
		document: {
			languageId: "active",
			uri: { scheme: "file", fsPath: fixture },
		},
	};

	const context = { subscriptions: [], extensionPath: __dirname };
	extension.activate(context);
	assert.strictEqual(statusCreated, 1, "status bar created");
	for (const command of [
		"active.check",
		"active.inspect",
		"active.graph",
		"active.repair",
		"active.capability",
		"active.refreshStatus",
	]) {
		assert.ok(registered.includes(command), `missing command ${command}`);
	}
	// Let the async refresh settle; poll so a cold runner does not flake.
	for (let i = 0; i < 50; i += 1) {
		if (
			statusItem.text.includes("Active config") ||
			statusItem.text.includes("Active data")
		) {
			break;
		}
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
	assert.ok(
		statusItem.text.includes("Active config") ||
			statusItem.text.includes("Active data"),
		`status refresh must drive the real CLI path, got: ${statusItem.text}`,
	);

	// Redaction at the editor boundary: the same CLI the extension calls
	// must not print a raw secret for an inspect/graph request.
	const exe = path.join(
		binDir,
		process.platform === "win32" ? "active.exe" : "active",
	);
	if (fs.existsSync(exe)) {
		const out = execFileSync(
			exe,
			["inspect", "--from", fixture, "--path", "token"],
			{
				encoding: "utf8",
			},
		);
		assert.ok(out.includes("***SECRET***"), "inspect redacts");
		assert.ok(
			!out.includes("hunter2"),
			"raw secret leaked to the editor surface",
		);
	} else {
		console.log("SMOKE: CLI absent, skipped redaction probe");
	}
	fs.unlinkSync(fixture);

	await extension.deactivate();
	console.log(
		`EDITOR-SMOKE: OK (${registered.length} commands, ${webviews} webviews)`,
	);
})().catch((error) => {
	console.error("EDITOR-SMOKE: FAILED", error);
	process.exit(1);
});
