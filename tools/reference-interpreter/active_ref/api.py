"""Parse/evaluate entry points used by the differential runner."""

from __future__ import annotations

from .diag import Diagnostic, ParseFailure
from .eval import Engine, EvalError, Evaluator, Outcome
from .includes import resolve_includes
from .parser import parse

# Include-loader failures make the document unevaluable in the shipped
# engine (no tree); E046 is a merge diagnostic and keeps its tree.
FATAL_INCLUDE = {"E015", "E016", "E017", "E028", "E041"}


def evaluate_source(
    source: str, name: str = "<source>", engine: Engine | None = None
) -> Outcome:
    engine = engine or Engine()
    try:
        module = parse(source, name)
    except ParseFailure as failure:
        return Outcome(None, list(failure.diagnostics))
    if module.includes:
        resolve_includes(module, name, engine)
    if any(d.code in FATAL_INCLUDE for d in module.diagnostics):
        return Outcome(None, list(module.diagnostics))
    try:
        return Evaluator(module, engine).run()
    except EvalError as error:
        diag = Diagnostic(
            error.code,
            "error",
            error.message,
            span=error.span,
            path=error.path,
            file=name,
        )
        return Outcome(None, [diag])


def evaluate_file(path: str, engine: Engine | None = None) -> Outcome:
    engine = engine or Engine()
    try:
        with open(path, encoding="utf-8") as handle:
            source = handle.read()
    except UnicodeDecodeError:
        return Outcome(
            None,
            [Diagnostic("E017", "error", f"cannot read `{path}`", file=path)],
        )
    except OSError:
        return Outcome(
            None,
            [Diagnostic("E017", "error", f"cannot read `{path}`", file=path)],
        )
    try:
        module = parse(source, path)
    except ParseFailure as failure:
        return Outcome(None, list(failure.diagnostics))
    resolve_includes(module, path, engine)
    if any(d.code in FATAL_INCLUDE for d in module.diagnostics):
        return Outcome(None, list(module.diagnostics))
    try:
        return Evaluator(module, engine).run()
    except EvalError as error:
        diag = Diagnostic(
            error.code,
            "error",
            error.message,
            span=error.span,
            path=error.path,
            file=path,
        )
        return Outcome(None, [diag])
