"""Active Python binding, WASM-first (CHECKLIST 1.004-1.006).

Zero native compilation: the binding drives the zero-import
``active_wasm.wasm`` module through wasmtime and exposes the agent-loop
surface with opaque handles, an ABI major gate, and Allocator Model A
(the module's ``active_free`` frees everything the module allocated).

Ownership rules: a ``Module`` owns one instantiated evaluator confined to
the thread that uses it. Use it as a context manager or call ``close()``
explicitly. Never share a module across threads without external locking.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import wasmtime

ABI_MAJOR = 1

_CODE_RE = re.compile(r"\b([A-Z][0-9]{3})\b")


class ActiveError(Exception):
    """Any failure surfaced by the Active module or the binding."""


class Diagnostic:
    """One structured diagnostic from an evaluation envelope."""

    def __init__(self, code: str, message: str) -> None:
        self.code = code
        self.message = message

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"Diagnostic({self.code}: {self.message[:80]})"


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

    def __init__(self, wasm_path: Path | None = None) -> None:
        path = wasm_path or default_wasm_path()
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
        return bytes(self._mem.read(self._store, ptr, ptr + size))

    def _u32(self, ptr: int) -> int:
        return int.from_bytes(self._read(ptr, 4), "little")

    def _call_eval(self, source: str) -> dict:
        src = source.encode("utf-8")
        sptr, sn = self._write(src)
        slot, _ = self._write(b"\x00" * 4)
        try:
            res = self._eval(self._store, sptr, sn, slot)
            env_len = self._u32(slot)
            raw = self._read(res, env_len)
            self._free(self._store, res, env_len)
            return json.loads(raw)
        finally:
            self._free(self._store, sptr, sn)
            self._free(self._store, slot, 4)

    def check(self, source: str) -> list[Diagnostic]:
        """Parse and evaluate; return structured diagnostics (empty if clean)."""
        env = self._call_eval(source)
        text = env.get("diagnostics", "") or ""
        return [
            Diagnostic(code, text) for code in dict.fromkeys(_CODE_RE.findall(text))
        ]

    def eval(self, source: str) -> dict:
        """Evaluate and return the redacted JSON value (raises on refusal)."""
        env = self._call_eval(source)
        if not env.get("ok", False):
            raise ActiveError(f"refused: {env.get('diagnostics', '')[:200]}")
        return env.get("json", {})
