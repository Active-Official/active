export interface ActiveDiagnosticSpan {
	readonly line: number;
	readonly column: number;
	readonly end_line: number;
	readonly end_column: number;
}

export interface ActiveDiagnostic {
	readonly code: string;
	readonly message: string;
	readonly severity: "error" | "warning" | "info" | "hint";
	/** Structured span, or null when the core left it out. */
	readonly span: ActiveDiagnosticSpan | null;
}

/**
 * L-P14: the shim returns the host's frozen copy. Both the record and the
 * nested diagnostic items are `Object.freeze`d at runtime, so a mutation
 * fails instead of silently corrupting a run; the readonly types express
 * the same contract to TypeScript.
 */
export interface ActiveEvaluateResult {
	readonly ok: boolean;
	readonly json: string;
	readonly diagnostics: string;
	/**
	 * Structured diagnostics, passed through verbatim when the module
	 * carries them and ABSENT otherwise: the bindings fail closed on a
	 * missing array, so the shim never invents an empty one.
	 */
	readonly items?: readonly ActiveDiagnostic[];
}

export interface ActiveAPI {
	evaluate(source: string): ActiveEvaluateResult;
	/**
	 * L-P05: evaluate under pure mode. Env reads, unpinned includes, and
	 * profile overrides are refused with E047 before any work. Throws when
	 * the loaded module predates the mode (fail loud, never evaluate
	 * impurely by accident).
	 */
	evaluatePure(source: string): ActiveEvaluateResult;
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
