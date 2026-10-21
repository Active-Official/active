"""Active Python binding, WASM-first (CHECKLIST 1.004-1.006).

Zero native compilation: the binding drives the zero-import
``active_wasm.wasm`` module through wasmtime and exposes the agent-loop
surface with opaque handles, an ABI major gate, and Allocator Model A
(the module's ``active_free`` frees everything the module allocated).

Ownership rules: a ``Module`` owns one instantiated evaluator confined to
the creating thread. One module per thread; never share a module across
threads, locking or not — the engine state is thread-confined by
construction. Use it as a context manager or call ``close()`` explicitly.
"""

from __future__ import annotations

import json
from pathlib import Path

import wasmtime

ABI_MAJOR = 1


class ActiveError(Exception):
    """Any failure surfaced by the Active module or the binding."""


class Diagnostic:
    """One structured diagnostic from an evaluation envelope.

    ``message`` is the full diagnostics text (kept for prose readers);
    ``line``/``column``/``end_line``/``end_column`` carry the structured
    span when the core attributed one (None when spanless), and
    ``severity`` is the core severity string (error/warning/info/hint).
    """

    def __init__(
        self,
        code: str,
        message: str,
        line: int | None = None,
        column: int | None = None,
        end_line: int | None = None,
        end_column: int | None = None,
        severity: str | None = None,
    ) -> None:
        self.code = code
        self.message = message
        self.line = line
        self.column = column
        self.end_line = end_line
        self.end_column = end_column
        self.severity = severity

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"Diagnostic({self.code}: {self.message[:80]})"

    def span(self) -> tuple[int, int, int, int] | None:
        """Structured span tuple, or None when the core left it spanless.

        Only complete spans are returned: a partial span (any slot
        missing) degrades to None rather than a tuple containing None,
        which callers treat identically to spanless.
        """
        parts = (self.line, self.column, self.end_line, self.end_column)
        if any(p is None for p in parts):
            return None
        return (self.line, self.column, self.end_line, self.end_column)  # type: ignore[return-value]


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def default_wasm_path() -> Path:
    return (
        _repo_root()
        / "target"
        / "wasm32-unknown-unknown"
        / "release"
        / "active_wasm.wasm"
    )


class Module:
    """One instantiated Active evaluator (opaque handle, do not share threads)."""

    def __init__(self, wasm_path: Path | str | None = None) -> None:
        path = Path(wasm_path) if wasm_path is not None else default_wasm_path()
        if not path.is_file():
            raise ActiveError(f"wasm module not found: {path}")
        self._store = wasmtime.Store()
        module = wasmtime.Module.from_file(self._store.engine, str(path))
        instance = wasmtime.Instance(self._store, module, [])
        exports = {e[0]: e[1] for e in instance.exports(self._store).items()}
        for name in (
            "active_alloc",
            "active_eval_json",
            "active_free",
            "active_abi_version",
        ):
            if name not in exports:
                raise ActiveError(f"module missing export: {name}")
        self._alloc = exports["active_alloc"]
        self._eval = exports["active_eval_json"]
        self._free = exports["active_free"]
        self._mem = instance.exports(self._store)["memory"]
        major = exports["active_abi_version"](self._store)
        if major != ABI_MAJOR:
            raise ActiveError(
                f"ABI major mismatch: module reports {major}, "
                f"binding expects {ABI_MAJOR}"
            )
        self._closed = False

    def __enter__(self) -> Module:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def close(self) -> None:
        self._closed = True

    def _write(self, data: bytes) -> tuple[int, int]:
        if self._closed:
            raise ActiveError("module is closed")
        ptr = self._alloc(self._store, len(data))
        self._mem.write(self._store, data, ptr)
        return ptr, len(data)

    def _read(self, ptr: int, size: int) -> bytes:
        # wasmtime read(store, start, stop) returns the byte range.
        try:
            return bytes(self._mem.read(self._store, ptr, ptr + size))
        except Exception as e:
            raise ActiveError(f"memory read failed: {e}") from e

    def _u32(self, ptr: int) -> int:
        try:
            return int.from_bytes(self._read(ptr, 4), "little")
        except ActiveError:
            raise
        except Exception as e:
            raise ActiveError(f"length slot unreadable: {e}") from e

    def _call_eval(self, source: str) -> dict:
        src = source.encode("utf-8")
        # Pointers start null so the finally below frees exactly what was
        # allocated: if _write raises (closed module), there is no module
        # allocation to free and no UnboundLocalError masks the real error.
        sptr: int | None = None
        sn: int = 0
        slot: int | None = None
        res: int = 0
        env_len: int = 0
        try:
            sptr, sn = self._write(src)
            slot, _ = self._write(b"\x00" * 4)
            try:
                res = self._eval(self._store, sptr, sn, slot)
            except Exception as e:
                # wasmtime traps (OOM, stack exhaustion) surface as the
                # binding's own error type, never a bare host exception.
                raise ActiveError(f"evaluation trapped: {e}") from e
            try:
                env_len = self._u32(slot)
                raw = self._read(res, env_len)
            finally:
                # Registry-backed free is size-agnostic; a zero length
                # here only means the length slot itself was unreadable.
                self._free(self._store, res, env_len)
            try:
                env = json.loads(raw)
            except Exception as e:
                raise ActiveError(f"envelope is not JSON: {e}") from e
            if not isinstance(env, dict):
                raise ActiveError("envelope is not an object")
            return env
        finally:
            if sptr is not None:
                self._free(self._store, sptr, sn)
            if slot is not None:
                self._free(self._store, slot, 4)

    def check(self, source: str) -> list[Diagnostic]:
        """Parse and evaluate; return structured diagnostics (empty if clean).

        The module runs the hermetic engine (resilient default), so this
        surfaces errors and repair warnings alike. One Diagnostic per
        envelope item, in core order (no dedup: recovery can legitimately
        repeat a code, and strict hosts compare the full list). Fail closed:
        an envelope without items is a version-skewed module, never a
        clean bill — returning [] here would be a false clean.
        """
        env = self._call_eval(source)
        if not isinstance(env.get("items"), list):
            raise ActiveError("envelope without items: rebuild active_wasm.wasm")
        text = env.get("diagnostics", "") or ""
        out = []
        for item in env["items"]:
            if not isinstance(item, dict) or not item.get("code"):
                # A code-less item is envelope corruption, not a clean
                # diagnostic: fail closed instead of emitting an empty code.
                raise ActiveError("envelope item without code")
            span = item.get("span")
            if span is not None and not isinstance(span, dict):
                raise ActiveError("envelope item with bad span")
            span = span or {}
            out.append(
                Diagnostic(
                    item["code"],
                    item.get("message") or text,
                    line=span.get("line"),
                    column=span.get("column"),
                    end_line=span.get("end_line"),
                    end_column=span.get("end_column"),
                    severity=item.get("severity"),
                )
            )
        return out

    def validate(self, source: str) -> list[Diagnostic]:
        """Validation step of the agent loop: the hermetic verdict.

        Returns the same structured diagnostics as check(): on this
        embedding the hermetic engine validates during evaluation, so
        check (diagnose) and validate (verify) share one verdict path
        and must always agree — the differential harness pins that.

        Budgets, three layers, all fail-closed:
        * iteration budget: enforced by the caller (reference loops pass
          max_iters and stop on stuck codes);
        * engine budgets: the hermetic engine ships hard defaults
          (nodes, depth, array length, wall clock); exceeding them
          refuses instead of truncating;
        * custom per-call budgets: not a parameter here by design. The
          zero-import WASM module runs fixed hermetic defaults so every
          host sees identical verdicts. Hosts that need custom budgets
          drive the C ABI surface (active_check with ActiveBudget),
          which is the same core with a budget pointer.
        """
        return self.check(source)

    def eval(self, source: str) -> dict:
        """Evaluate and return the redacted JSON value (raises on refusal).

        A malformed ok:true envelope (missing or non-dict value) raises
        instead of returning a plausible-looking empty dict.
        """
        env = self._call_eval(source)
        if not env.get("ok", False):
            raise ActiveError(f"refused: {env.get('diagnostics', '')[:200]}")
        value = env.get("json")
        if not isinstance(value, dict):
            raise ActiveError("envelope ok without a JSON object value")
        return value
