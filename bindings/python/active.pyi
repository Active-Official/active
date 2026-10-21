"""Type stubs for the Active Python binding (Phase A, PROPOSAL §1.2).

Mirrors bindings/python/active.py exactly: same public functions,
classes, methods, and signatures. test_active.py::test_stub_matches_runtime
pins the mapping (names, arity, and defaults); keep both files in sync.
"""

from pathlib import Path


ABI_MAJOR: int


def default_wasm_path() -> Path: ...


class ActiveError(Exception): ...


class Diagnostic:
    code: str
    message: str
    line: int | None
    column: int | None
    end_line: int | None
    end_column: int | None
    severity: str | None
    def __init__(
        self,
        code: str,
        message: str,
        line: int | None = ...,
        column: int | None = ...,
        end_line: int | None = ...,
        end_column: int | None = ...,
        severity: str | None = ...,
    ) -> None: ...
    def span(self) -> tuple[int, int, int, int] | None: ...


class Module:
    def __init__(self, wasm_path: Path | str | None = ...) -> None: ...
    def __enter__(self) -> Module: ...
    def __exit__(self, *exc: object) -> None: ...
    def close(self) -> None: ...
    def check(self, source: str) -> list[Diagnostic]: ...
    def validate(self, source: str) -> list[Diagnostic]: ...
    def eval(self, source: str) -> dict: ...
