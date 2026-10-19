export interface ActiveEvaluateResult {
	ok: boolean;
	json: string;
	diagnostics: string;
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
