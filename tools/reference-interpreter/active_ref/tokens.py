"""Token kinds and spans for the L-P37 lexer."""

from __future__ import annotations

from dataclasses import dataclass

from .diag import Span


@dataclass(frozen=True)
class StringPart:
    kind: str  # "lit" | "expr"
    text: str  # literal text, or the inner expression source
    span: Span


@dataclass
class Token:
    kind: str
    value: object = None
    span: Span | None = None
    unit: str | None = None
    parts: list[StringPart] | None = None

    def __repr__(self) -> str:  # test/debug only
        return f"Token({self.kind}, {self.value!r}, unit={self.unit!r})"


@dataclass
class Comment:
    line: int
    text: str
    trailing: bool


# Token kinds.
NEWLINE = "newline"
INDENT = "indent"
DEDENT = "dedent"
EOF = "eof"
IDENT = "ident"
INT = "int"
FLOAT = "float"
STRING = "string"
BOOL = "bool"
NULL = "null"
OP = "op"
PUNCT = "punct"
AT = "at"
PIN = "pin"
