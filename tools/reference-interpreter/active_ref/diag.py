"""Diagnostic records and the stable code vocabulary (L-P37).

The differential compares the structured record (code, severity, path,
span, order), never prose: D-1 as amended by Bud's G01 decision.
"""

from __future__ import annotations

from dataclasses import dataclass, field

ERROR = "error"
WARNING = "warning"
INFO = "info"


@dataclass(frozen=True)
class Span:
    line: int
    column: int
    end_line: int
    end_column: int

    def as_tuple(self) -> tuple[int, int, int, int]:
        return (self.line, self.column, self.end_line, self.end_column)


@dataclass
class Diagnostic:
    code: str
    severity: str
    message: str
    path: str | None = None
    span: Span | None = None
    file: str | None = None

    def key(self) -> tuple:
        return (
            self.code,
            self.severity,
            self.path,
            self.span.as_tuple() if self.span else None,
        )


class ActiveError(Exception):
    """A hard diagnostic that ends parsing or evaluation."""

    def __init__(self, diagnostic: Diagnostic):
        super().__init__(diagnostic.message)
        self.diagnostic = diagnostic


class ParseFailure(Exception):
    """A parse that produced no module; carries the ordered diagnostics."""

    def __init__(self, diagnostics: list[Diagnostic]):
        super().__init__(diagnostics[0].message if diagnostics else "parse failed")
        self.diagnostics = diagnostics


def error(
    code: str,
    message: str,
    *,
    path: str | None = None,
    span: Span | None = None,
    file: str | None = None,
) -> Diagnostic:
    return Diagnostic(code, ERROR, message, path, span, file)


def warning(
    code: str,
    message: str,
    *,
    path: str | None = None,
    span: Span | None = None,
    file: str | None = None,
) -> Diagnostic:
    return Diagnostic(code, WARNING, message, path, span, file)


def info(
    code: str,
    message: str,
    *,
    path: str | None = None,
    span: Span | None = None,
    file: str | None = None,
) -> Diagnostic:
    return Diagnostic(code, INFO, message, path, span, file)


@dataclass
class DiagnosticBag:
    items: list[Diagnostic] = field(default_factory=list)

    def add(self, diagnostic: Diagnostic) -> None:
        self.items.append(diagnostic)

    def error(self, code: str, message: str, **kwargs) -> None:
        self.add(error(code, message, **kwargs))

    def warning(self, code: str, message: str, **kwargs) -> None:
        self.add(warning(code, message, **kwargs))

    def has_error(self) -> bool:
        return any(d.severity == ERROR for d in self.items)

    def sorted(self) -> list[Diagnostic]:
        # G02: phase order is preserved by the producer; this helper only
        # stabilizes ties inside one phase by (path, code, span).
        return sorted(
            self.items,
            key=lambda d: (
                d.path or "",
                d.code,
                d.span.as_tuple() if d.span else (0, 0, 0, 0),
            ),
        )
