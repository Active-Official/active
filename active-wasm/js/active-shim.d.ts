export interface ActiveDiagnosticSpan {
	line: number;
	column: number;
	end_line: number;
	end_column: number;
}

export interface ActiveDiagnostic {
	code: string;
	message: string;
	severity: "error" | "warning" | "info" | "hint";
	/** Structured span, or null when the core left it out. */
	span: ActiveDiagnosticSpan | null;
}

export interface ActiveEvaluateResult {
	ok: boolean;
	json: string;
	diagnostics: string;
	/**
	 * Structured diagnostics, passed through verbatim when the module
	 * carries them and ABSENT otherwise: the bindings fail closed on a
	 * missing array, so the shim never invents an empty one.
	 */
	items?: ActiveDiagnostic[];
}

export interface ActiveAPI {
	evaluate(source: string): ActiveEvaluateResult;
	abiVersion: 1;
}

export function load(
	source: string | ArrayBuffer | Response,
): Promise<ActiveAPI>;

declare global {
	interface Window {
		ActiveAPI?: ActiveAPI;
		ActiveShim?: { load: typeof load };
	}
}
