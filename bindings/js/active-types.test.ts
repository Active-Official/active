// Type-shape test for active.d.ts: exercises every exported member.
// Checked with `tsc --noEmit` (see bindings/js run notes).
import { type Diagnostic, load, type Module } from "./active";

async function main(source: string): Promise<unknown> {
	const m: Module = await load();
	return drive(m, source);
}

async function drive(mod: Module, source: string): Promise<unknown> {
	const diags: Diagnostic[] = mod.check(source);
	const recheck: Diagnostic[] = mod.validate(source);
	for (const d of [...diags, ...recheck]) {
		const code: string = d.code;
		const message: string = d.message;
		const line: number | null = d.line;
		const column: number | null = d.column;
		const endLine: number | null = d.end_line;
		const endColumn: number | null = d.end_column;
		const severity: "error" | "warning" | "info" | "hint" | null = d.severity;
		void [code, message, line, column, endLine, endColumn, severity];
	}
	const value: unknown = mod.evaluate(source);
	mod.close();
	return value;
}

void main;
void drive;
