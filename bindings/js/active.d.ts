/**
 * TypeScript types for the Active JS/TS binding (Phase A, PROPOSAL §1.3).
 *
 * Mirrors bindings/js/active.js exactly (runtime exports `{ load, Module }`).
 * test_active.js exercises the runtime surface and active-types.test.ts
 * binds every type; keep this file in sync when the runtime changes
 * (same names, same shapes, same nullability).
 */

/** One structured diagnostic from a check() call. */
export interface Diagnostic {
	code: string;
	/** Full diagnostics text (prose, for humans). */
	message: string;
	/** 1-based line, or null when the core left the span out. */
	line: number | null;
	column: number | null;
	end_line: number | null;
	end_column: number | null;
	/** Core severity, or null when absent. */
	severity: "error" | "warning" | "info" | "hint" | null;
}

/** Opaque Active evaluator. One instance per task; do not share threads. */
export declare class Module {
	/**
	 * Parse and evaluate; return structured diagnostics (empty if clean).
	 * One entry per envelope item, in core order (no dedup).
	 */
	check(source: string): Diagnostic[];
	/**
	 * Validation step of the agent loop: same hermetic verdict as check().
	 * Kept as a separate entry point so portable agent code can call
	 * validate() on every Tier-1 binding.
	 */
	validate(source: string): Diagnostic[];
	/** Evaluate and return the redacted JSON value. Throws on refusal. */
	evaluate(source: string): unknown;
	/** Release the module. Using it afterwards throws. */
	close(): void;
}

/** Alias for consumers that import the instance type by name. */
export type ActiveModule = Module;

/**
 * Load the shipped WASM module (default path) or the module at the given
 * filesystem path, and return a ready instance. Load once and reuse the
 * module across calls.
 */
export declare function load(wasmPath?: string): Promise<ActiveModule>;
