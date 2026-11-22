"""Recursive-descent parser for the Active surface (spec grammar §2).

Valid-document coverage first: both layouts, expressions, units, schemas,
profiles, includes, patches, annotations. Malformed-input recovery is
implemented only where the corpus needs it; otherwise the parse fails
with the shipped code (gap-blocked in the differential).
"""

from __future__ import annotations

import math
import sys
from dataclasses import dataclass, field
from typing import Any

from . import tokens as T
from .diag import Diagnostic, ParseFailure, Span, error, warning
from .lexer import lex
from .model import (
    Annotation,
    IncludeStmt,
    Module,
    Node,
    Patch,
    SchemaField,
    SchemaTable,
    SchemaType,
    Value,
)
from .units import I64_MAX, UNITS

I64_MIN = -(2**63)


@dataclass
class TBuilder:
    fields: dict[str, Any] = field(default_factory=dict)
    spans: dict[str, Span] = field(default_factory=dict)
    parent: Scope | None = None


@dataclass
class Scope:
    parent: Scope | None
    fields: dict[str, int] = field(default_factory=dict)
    children: dict[str, Scope] = field(default_factory=dict)


class Cursor:
    def __init__(self, tokens: list[T.Token]):
        self.tokens = tokens
        self.i = 0

    def peek(self, offset: int = 0) -> T.Token:
        j = self.i + offset
        if j >= len(self.tokens):
            return self.tokens[-1]
        return self.tokens[j]

    def next(self) -> T.Token:
        tok = self.peek()
        if self.i < len(self.tokens) - 1:
            self.i += 1
        return tok

    def at(self, kind: str, value: object = None) -> bool:
        tok = self.peek()
        return tok.kind == kind and (value is None or tok.value == value)

    def at_punct(self, value: str, offset: int = 0) -> bool:
        tok = self.peek(offset)
        return tok.kind == T.PUNCT and tok.value == value

    def at_op(self, value: str, offset: int = 0) -> bool:
        tok = self.peek(offset)
        return tok.kind == T.OP and tok.value == value

    def accept_punct(self, value: str) -> bool:
        if self.at_punct(value):
            self.next()
            return True
        return False

    def expect_punct(self, value: str) -> T.Token:
        if not self.at_punct(value):
            raise self.fail("E001", f"expected `{value}`", self.peek())
        return self.next()

    def expect(self, kind: str, value: object = None, code: str = "E001") -> T.Token:
        if not self.at(kind, value):
            raise self.fail(code, f"expected {kind}", self.peek())
        return self.next()

    def skip_newlines(self) -> None:
        while self.at(T.NEWLINE):
            self.next()

    def fail(self, code: str, message: str, tok: T.Token) -> ParseFailure:
        return ParseFailure([error(code, message, span=tok.span, file="<source>")])


class Parser:
    def __init__(self, source: str, name: str = "<source>"):
        self.source = source
        self.name = name
        result = lex(source, name)
        self.diagnostics: list[Diagnostic] = list(result.diagnostics)
        self.comments = result.comments
        self.cur = Cursor(result.tokens)
        self.module = Module(source_name=name, source=source)
        self.root_builder = TBuilder()
        self.current = self.root_builder
        self.current_path: list[str] = []
        self.header_scope: TBuilder | None = None
        self.pending_annotations: list[Annotation] = []
        self.nesting = 0
        self.repeated_bodies: set[int] = set()
        self.block_bases: list[TBuilder] = []

    # -- entry ------------------------------------------------------------
    def parse(self) -> Module:
        self._document(body_end=None)
        self.module.root = self._materialize(self.root_builder, None, "")
        self._resolve_all()
        self._attach_immutables()
        diags = list(self.diagnostics) + list(self.module.diagnostics)
        if len(diags) > 1024:
            diags = diags[:1023]
            diags.append(
                warning(
                    "W014",
                    "further diagnostics truncated: the 1024-diagnostic output budget was reached",
                    file=self.name,
                )
            )
        self.module.diagnostics = diags
        # E046 from a repeated header is a merge diagnostic, not a parse
        # refusal: the shipped engine evaluates the first body and reports
        # the conflict with a value tree.
        if any(d.severity == "error" and d.code != "E046" for d in diags):
            raise ParseFailure(diags)
        return self.module

    def _document(self, body_end: str | None) -> None:
        while True:
            self.cur.skip_newlines()
            if self.cur.at(T.EOF) or (body_end and self.cur.at_punct(body_end)):
                return
            if self.cur.at(T.DEDENT):
                if body_end == "dedent":
                    return
                self.cur.next()
                continue
            if self.cur.at_punct("}"):
                if body_end == "}":
                    return
                self.cur.next()
                continue
            try:
                self._statement()
            except ParseFailure as failure:
                self.diagnostics.extend(failure.diagnostics)
                self._skip_to_newline()

    def _skip_to_newline(self) -> None:
        while not self.cur.at(T.NEWLINE) and not self.cur.at(T.EOF):
            self.cur.next()
        if self.cur.at(T.NEWLINE):
            self.cur.next()

    # -- statements -------------------------------------------------------
    def _statement(self) -> None:
        tok = self.cur.peek()
        if tok.kind == T.IDENT and tok.value == "let":
            self._let()
            return
        if tok.kind == T.IDENT and tok.value == "schema":
            self._schema_statement()
            return
        if (
            tok.kind == T.IDENT
            and tok.value == "profile"
            and self.cur.peek(1).kind == T.STRING
        ):
            self._profile()
            return
        if tok.kind == T.IDENT and tok.value == "include":
            self._include()
            return
        if tok.kind == T.AT:
            self._at_statement()
            return
        if tok.kind == T.PUNCT and tok.value == "[":
            self._header()
            return
        if tok.kind == T.STRING:
            # A quoted key is not an identifier: the shipped parser reports
            # the key string and then the next string token on the line
            # (the value), then skips the rest (family 4 recovery, pinned
            # by the MC dump).
            second = tok.span
            offset = 1
            while True:
                nxt = self.cur.peek(offset)
                if nxt.kind in (T.NEWLINE, T.EOF):
                    break
                if nxt.kind == T.STRING:
                    second = nxt.span
                    break
                offset += 1
            for index in range(2):
                self.diagnostics.append(
                    error(
                        "E002",
                        "quoted key is not an identifier",
                        span=tok.span if index == 0 else second,
                        file=self.name,
                    )
                )
            self._skip_to_newline()
            return
        if tok.kind == T.IDENT:
            # Table block / indentation header / key-value.
            if self.cur.at_punct("{", 1):
                name = self.cur.next().value
                self._table_block(name, tok.span)
                return
            if self.cur.at_punct(":", 1) and self.cur.peek(2).kind == T.NEWLINE:
                # `name:` + newline is a block; `name: value` is a key-value.
                name = self.cur.next().value
                self.cur.next()  # ':'
                self.cur.next()  # newline
                self._expect_indent()
                self._block_body(self._child(name, tok.span))
                return
            self._key_value()
            return
        raise ParseFailure(
            [
                error(
                    "E002",
                    "unexpected token at statement position",
                    span=tok.span,
                    file=self.name,
                )
            ]
        )

    def _expect_indent(self) -> None:
        if not self.cur.at(T.INDENT):
            tok = self.cur.peek()
            raise ParseFailure(
                [
                    error(
                        "E008",
                        "header has no indented body",
                        span=tok.span,
                        file=self.name,
                    )
                ]
            )
        self.cur.next()

    def _block_body(self, builder: TBuilder) -> None:
        saved = self.current
        self.current = builder
        self.block_bases.append(builder)
        self._document("dedent")
        self.block_bases.pop()
        if self.cur.at(T.DEDENT):
            self.cur.next()
        self.current = saved

    def _let(self) -> None:
        self.cur.next()  # let
        name_tok = self.cur.expect(T.IDENT, code="E001")
        if not self.cur.accept_punct("="):
            raise ParseFailure(
                [
                    error(
                        "E001",
                        "expected `=` after let name",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        value = self._expression()
        self.module.bindings[name_tok.value] = value
        self.current = self.root_builder
        self.current_path = []

    def _include(self) -> None:
        self.cur.next()  # include
        if self.cur.peek().kind != T.STRING:
            raise ParseFailure(
                [
                    error(
                        "E015",
                        "include path must be a string literal",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        path_tok = self.cur.next()
        path = _string_value(path_tok, self)
        if "\\" in path or (
            len(path) >= 3
            and path[0].isalpha()
            and path[1] == ":"
            and path[2] in ("/", "\\")
        ):
            raise ParseFailure(
                [
                    error(
                        "E026",
                        "include path is not portable across hosts",
                        span=path_tok.span,
                        file=self.name,
                    )
                ]
            )
        pin = None
        alias = None
        private = False
        while True:
            if self.cur.peek().kind == T.PIN:
                if pin is not None:
                    raise ParseFailure(
                        [
                            error(
                                "E015",
                                "duplicate include pin",
                                span=self.cur.peek().span,
                                file=self.name,
                            )
                        ]
                    )
                pin = self.cur.next().value
                continue
            if self.cur.at(T.IDENT, "as"):
                if alias is not None:
                    raise ParseFailure(
                        [
                            error(
                                "E015",
                                "duplicate include alias",
                                span=self.cur.peek().span,
                                file=self.name,
                            )
                        ]
                    )
                self.cur.next()
                alias = self.cur.expect(T.IDENT).value
                continue
            if self.cur.at(T.IDENT, "private"):
                if private:
                    raise ParseFailure(
                        [
                            error(
                                "E015",
                                "duplicate include option `private`",
                                span=self.cur.peek().span,
                                file=self.name,
                            )
                        ]
                    )
                self.cur.next()
                private = True
                continue
            break
        self.module.includes.append(
            IncludeStmt(path, pin, alias, private, path_tok.span)
        )

    def _at_statement(self) -> None:
        at = self.cur.next()
        kind_tok = self.cur.expect(T.IDENT)
        kind = kind_tok.value
        if kind in ("patch", "remove", "rename"):
            self._patch(kind, at.span)
            return
        # Semantic tag at statement level: `@prompt """..."""` -> key prompt.
        if self.cur.peek().kind == T.STRING:
            value = self._primary()
            self._set(kind, value, kind_tok.span)
            return
        raise ParseFailure(
            [
                error(
                    "E014",
                    f"unknown statement tag `@{kind}`",
                    span=kind_tok.span,
                    file=self.name,
                )
            ]
        )

    def _patch(self, kind: str, span: Span | None) -> None:
        self.cur.expect_punct("(")
        if self.cur.peek().kind != T.STRING:
            raise ParseFailure(
                [
                    error(
                        "E019",
                        "patch path must be a string literal",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        path_tok = self.cur.next()
        path = _string_value(path_tok, self)
        if kind == "patch":
            self.cur.expect_punct(",")
            value = self._expression()
            self.cur.expect_punct(")")
            self.module.patches.append(Patch("patch", path, value=value, span=span))
        elif kind == "remove":
            self.cur.expect_punct(")")
            self.module.patches.append(Patch("remove", path, span=span))
        else:
            self.cur.expect_punct(",")
            new_tok = self.cur.expect(T.STRING)
            self.cur.expect_punct(")")
            self.module.patches.append(
                Patch("rename", path, new_name=_string_value(new_tok, self), span=span)
            )

    def _header(self) -> None:
        self.cur.expect_punct("[")
        segments = [self.cur.expect(T.IDENT).value]
        while self.cur.accept_punct("."):
            segments.append(self.cur.expect(T.IDENT).value)
        self.cur.expect_punct("]")
        builder = self.block_bases[-1] if self.block_bases else self.root_builder
        for index, segment in enumerate(segments):
            existing = builder.fields.get(segment)
            builder = self._child_into(builder, segment, None)
            if index == len(segments) - 1 and isinstance(existing, TBuilder):
                self.repeated_bodies.add(id(builder))
        self.current = builder
        self.current_path = segments
        path = ".".join(str(segment) for segment in segments)
        if path and self.cur.peek().span is not None:
            self.module.value_spans.setdefault(path, self.cur.peek().span)

    def _table_block(self, name: str, span: Span | None) -> None:
        self.cur.expect_punct("{")
        builder = self._child(name, span)
        path = ".".join([*self.current_path, name])
        if span is not None:
            self.module.value_spans.setdefault(path, span)
        saved = self.current
        saved_path = self.current_path
        self.current = builder
        self.current_path = [*self.current_path, name]
        self.block_bases.append(builder)
        try:
            self._document("}")
        finally:
            self.block_bases.pop()
            self.current_path = saved_path
        self.cur.expect_punct("}")
        self.current = saved

    def _key_value(self) -> None:
        name_tok = self.cur.expect(T.IDENT)
        if not (self.cur.accept_punct("=") or self.cur.accept_punct(":")):
            raise ParseFailure(
                [
                    error(
                        "E001",
                        f"expected `=` after `{name_tok.value}`",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        value = self._expression()
        self._set(name_tok.value, value, name_tok.span)
        path = ".".join([*self.current_path, name_tok.value])
        self._record_value_spans(value, path)
        last = self.cur.tokens[self.cur.i - 1]
        line = last.span.end_line if last.span else None
        while True:
            tok = self.cur.peek()
            if tok.kind in (T.NEWLINE, T.DEDENT, T.EOF):
                break
            if tok.kind == T.PUNCT and tok.value == "}":
                break
            if line is not None and tok.span is not None and tok.span.line != line:
                break
            if tok.kind in (T.INT, T.FLOAT) or (
                tok.kind == T.PUNCT and tok.value == "."
            ):
                # The pinned `x = 5 5` / `x = 1.` quirks skip silently.
                self.cur.next()
                continue
            self.module.diagnostics.append(
                warning(
                    "W015",
                    "a completed statement is followed by another token on the same line",
                    span=tok.span,
                    file=self.name,
                )
            )
            break

    # -- table builders ---------------------------------------------------
    def _child(self, name: str, span: Span | None) -> TBuilder:
        return self._child_into(self.current, name, span)

    def _child_into(self, builder: TBuilder, name: str, span: Span | None) -> TBuilder:
        existing = builder.fields.get(name)
        if isinstance(existing, TBuilder):
            return existing
        if existing is not None:
            raise ParseFailure(
                [
                    error(
                        "E002",
                        f"`{name}` is already a value",
                        span=span,
                        file=self.name,
                    )
                ]
            )
        child = TBuilder()
        builder.fields[name] = child
        if span:
            builder.spans[name] = span
        return child

    def _record_value_spans(self, node_id: int, path: str) -> None:
        node = self.module.get(node_id)
        if node.span is not None:
            self.module.value_spans[path] = node.span
        if node.kind == "table":
            for name, child in node.fields.items():
                self._record_value_spans(child, _join(path, name))
        elif node.kind == "array":
            for child in node.children:
                self._record_value_spans(child, f"{path}[]")

    def _set(self, name: str, value: int, span: Span | None) -> None:
        if name in self.current.fields:
            old = self.current.fields[name]
            if id(self.current) in self.repeated_bodies:
                old_node = self.module.get(old) if isinstance(old, int) else None
                new_node = self.module.get(value)
                same = (
                    old_node is not None
                    and old_node.kind == "literal"
                    and new_node.kind == "literal"
                    and old_node.value == new_node.value
                )
                if not same:
                    self.diagnostics.append(
                        error(
                            "E046",
                            f"merge conflict at `{name}`",
                            path=_join(".".join(self.current_path), name),
                            span=(new_node.span if new_node is not None else span),
                            file=self.name,
                        )
                    )
                    # First source wins (spec §3.4): do not overwrite the
                    # kept value with the conflicting one.
                    return
            else:
                self.diagnostics.append(
                    warning(
                        "W011",
                        f"duplicate key `{name}`; last wins",
                        span=span,
                        file=self.name,
                    )
                )
        self.current.fields[name] = value
        if span:
            self.current.spans[name] = span

    # -- profiles ---------------------------------------------------------
    def _profile(self) -> None:
        keyword = self.cur.next()  # profile
        name_tok = self.cur.expect(T.STRING)
        name = _string_value(name_tok, self)
        if name in self.module.profiles:
            raise ParseFailure(
                [
                    error(
                        "E018",
                        f"duplicate profile `{name}`",
                        span=name_tok.span,
                        file=self.name,
                    )
                ]
            )
        if self.cur.at_punct("{"):
            self.cur.next()
            builder = TBuilder()
            saved = self.current
            self.current = builder
            self.block_bases.append(builder)
            self._document("}")
            self.block_bases.pop()
            self.cur.expect_punct("}")
            self.current = saved
        else:
            raise ParseFailure(
                [
                    error(
                        "E018",
                        "profile body must be a block",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        for meta in ("active_mode", "active_capabilities"):
            if meta in builder.fields:
                raise ParseFailure(
                    [
                        error(
                            "E027",
                            f"profile metadata rewrite `{meta}` is not allowed",
                            file=self.name,
                        )
                    ]
                )
        # Store the builder on the module; materialized after the root.
        self.module.profiles[name] = -1  # placeholder
        self.module.profile_spans[name] = keyword.span
        self._profile_builders = getattr(self, "_profile_builders", {})
        self._profile_builders[name] = builder

    # -- schema -----------------------------------------------------------
    def _schema_statement(self) -> None:
        keyword = self.cur.next()  # schema
        self.module.schema_span = keyword.span
        if self.cur.accept_punct("="):
            if not self.cur.at_punct("{"):
                raise ParseFailure(
                    [
                        error(
                            "E020",
                            "schema value must be a table",
                            span=self.cur.peek().span,
                            file=self.name,
                        )
                    ]
                )
            schema = self._schema_table()
        elif self.cur.at_punct("{"):
            schema = self._schema_table()
        elif self.cur.accept_punct(":"):
            self.cur.skip_newlines()
            self._expect_indent()
            schema = self._schema_entries_body()
        else:
            raise ParseFailure(
                [
                    error(
                        "E020",
                        "malformed schema block",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        self.module.schema = schema
        self.module.schema_node = None

    def _schema_table(self) -> SchemaTable:
        self.cur.expect_punct("{")
        schema = SchemaTable()
        self._schema_entries(schema, "}")
        self.cur.expect_punct("}")
        return schema

    def _schema_entries_body(self) -> SchemaTable:
        schema = SchemaTable()
        self._schema_entries(schema, "dedent")
        if self.cur.at(T.DEDENT):
            self.cur.next()
        return schema

    def _schema_entries(self, schema: SchemaTable, end: str) -> None:
        while True:
            self.cur.skip_newlines()
            if end == "}" and self.cur.at_punct("}"):
                return
            if end == "dedent" and self.cur.at(T.DEDENT):
                return
            if self.cur.at(T.EOF):
                return
            if self.cur.at(T.IDENT, "check"):
                self.cur.next()
                if self.cur.at_punct("{"):
                    self.cur.next()
                    self._check_entries(schema, "}")
                    self.cur.expect_punct("}")
                elif self.cur.accept_punct(":"):
                    self.cur.skip_newlines()
                    self._expect_indent()
                    self._check_entries(schema, "dedent")
                    if self.cur.at(T.DEDENT):
                        self.cur.next()
                else:
                    raise ParseFailure(
                        [
                            error(
                                "E020",
                                "malformed check block",
                                span=self.cur.peek().span,
                                file=self.name,
                            )
                        ]
                    )
                continue
            name_tok = self.cur.expect(T.IDENT)
            self.cur.expect_punct("=")
            if (
                name_tok.value in ("version", "min_reader", "min_writer")
                and self.cur.peek().kind == T.STRING
            ):
                value_tok = self.cur.next()
                value = _string_value(value_tok, self)
                slot = "schema_" + name_tok.value
                if not _is_schema_version(value):
                    self.diagnostics.append(
                        error(
                            "E020",
                            f"{name_tok.value} must be major.minor.patch",
                            span=value_tok.span,
                            file=self.name,
                        )
                    )
                elif getattr(self.module, slot, None) is None:
                    setattr(self.module, slot, value)
                if self.cur.accept_punct(","):
                    continue
                self.cur.skip_newlines()
                continue
            field = self._schema_type()
            schema.fields[name_tok.value] = field
            if self.cur.accept_punct(","):
                continue
            if end == "}" and self.cur.at_punct("}"):
                return
            self.cur.skip_newlines()

    def _check_entries(self, schema: SchemaTable, end: str) -> None:
        while True:
            self.cur.skip_newlines()
            if end == "}" and self.cur.at_punct("}"):
                return
            if end == "dedent" and self.cur.at(T.DEDENT):
                return
            if self.cur.at(T.EOF):
                return
            name_tok = self.cur.expect(T.IDENT)
            name = name_tok.value
            self.cur.expect_punct("=")
            expr = self._expression()
            schema.checks.append((name, expr))
            self.module.check_spans[expr] = name_tok.span
            if self.cur.accept_punct(","):
                continue

    def _schema_type(self) -> SchemaField:
        tok = self.cur.peek()
        if tok.kind != T.IDENT:
            raise ParseFailure(
                [error("E020", "expected a schema type", span=tok.span, file=self.name)]
            )
        name = self.cur.next().value
        field = SchemaField(SchemaType(name), span=tok.span)
        if name == "Array":
            self.cur.expect_punct("<")
            inner = self._schema_type()
            self.cur.expect_punct(">")
            field.typ = SchemaType("Array", element=inner.typ)
            return field
        if name in ("Table", "OneOf", "Enum"):
            if name == "Table" and self.cur.at_punct("{"):
                field.typ = self._schema_table_type()
            elif name == "Enum":
                self.cur.expect_punct("(")
                while self.cur.peek().kind == T.STRING:
                    field.typ.members.append(_string_value(self.cur.next(), self))
                    if not self.cur.accept_punct(","):
                        break
                self._schema_named_args(field)
                self.cur.expect_punct(")")
            elif name == "OneOf":
                self.cur.expect_punct("(")
                branches = []
                while True:
                    if self.cur.peek().kind == T.IDENT and self.cur.at_punct("=", 1):
                        break
                    branches.append(self._schema_type())
                    if not self.cur.accept_punct(","):
                        break
                field.typ = SchemaType("OneOf", branches=branches)
                self._schema_named_args(field)
                self.cur.expect_punct(")")
        if self.cur.at_punct("("):
            self.cur.next()
            self._schema_named_args(field)
            self.cur.expect_punct(")")
        return field

    def _schema_table_type(self) -> SchemaType:
        self.cur.expect_punct("{")
        typ = SchemaType("Table")
        while not self.cur.at_punct("}"):
            self.cur.skip_newlines()
            if self.cur.at_punct("}"):
                break
            name_tok = self.cur.expect(T.IDENT)
            name = name_tok.value
            self.cur.expect_punct("=")
            if name == "closed":
                tok = self.cur.peek()
                if tok.kind == T.BOOL:
                    typ.closed = bool(self.cur.next().value)
                else:
                    raise ParseFailure(
                        [
                            error(
                                "E020",
                                "closed must be a boolean",
                                span=tok.span,
                                file=self.name,
                            )
                        ]
                    )
            else:
                field = self._schema_type()
                typ.fields[name] = field
            if not self.cur.accept_punct(","):
                self.cur.skip_newlines()
        self.cur.expect_punct("}")
        return typ

    def _schema_named_args(self, field: SchemaField) -> None:
        while self.cur.peek().kind == T.IDENT and self.cur.at_punct("=", 1):
            name = self.cur.next().value
            self.cur.next()  # '='
            arg = self._expression()
            field.args[name] = arg
            if not self.cur.accept_punct(","):
                break
        if "description" in field.args:
            field.description = _literal_string(self.module, field.args["description"])
        if "deprecated" in field.args:
            field.deprecated = (
                _literal_string(self.module, field.args["deprecated"]) or ""
            )
        if "removed_in" in field.args:
            removed = _literal_string(self.module, field.args["removed_in"])
            if removed is not None and _is_schema_version(removed):
                field.removed_in = removed
            else:
                node = self.module.get(field.args["removed_in"])
                self.diagnostics.append(
                    error(
                        "E020",
                        "removed_in takes a version string (major.minor.patch)",
                        span=node.span,
                        file=self.name,
                    )
                )
        if "requires" in field.args:
            field.requires = _literal_string(self.module, field.args["requires"])
        if "media_type" in field.args:
            field.blob_media_type = _literal_string(
                self.module, field.args["media_type"]
            )
        if "max_size" in field.args:
            node = (
                self.module.get(field.args["max_size"])
                if field.args["max_size"] < len(self.module.nodes)
                else None
            )
            if node and node.kind == "literal" and node.value.kind == "int":
                field.blob_max_size = node.value.data

    # -- expressions ------------------------------------------------------
    def _expression(self) -> int:
        return self._or()

    def _or(self) -> int:
        node = self._and()
        while self.cur.at_op("||"):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._and()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _and(self) -> int:
        node = self._eq()
        while self.cur.at_op("&&"):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._eq()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _eq(self) -> int:
        node = self._cmp()
        while self.cur.at_op("==") or self.cur.at_op("!="):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._cmp()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _cmp(self) -> int:
        node = self._term()
        while (
            self.cur.at_op("<")
            or self.cur.at_op("<=")
            or self.cur.at_op(">")
            or self.cur.at_op(">=")
        ):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._term()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _term(self) -> int:
        node = self._factor()
        while self.cur.at_op("+") or self.cur.at_op("-"):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._factor()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _factor(self) -> int:
        node = self._unary()
        while self.cur.at_op("*") or self.cur.at_op("/"):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            rhs = self._unary()
            node = self.module.add(
                Node(
                    "binary",
                    op=op,
                    children=[node, rhs],
                    span=op_tok.span,
                )
            )
        return node

    def _unary(self) -> int:
        if self.cur.at_op("-") or self.cur.at_op("!"):
            op_tok = self.cur.peek()
            op = self.cur.next().value
            expr = self._unary()
            return self.module.add(
                Node(
                    "unary",
                    op=op,
                    children=[expr],
                    span=op_tok.span,
                )
            )
        return self._primary()

    def _primary(self) -> int:
        tok = self.cur.peek()
        if tok.kind == T.STRING:
            self.cur.next()
            return self._string_node(tok)
        if tok.kind in (T.INT, T.FLOAT):
            self.cur.next()
            if tok.unit:
                return self.module.add(
                    Node(
                        "quantity",
                        value=float(tok.value),
                        unit=UNITS[tok.unit],
                        span=tok.span,
                    )
                )
            if tok.kind == T.INT:
                if tok.value > I64_MAX:
                    self.diagnostics.append(
                        error(
                            "E023",
                            "integer literal out of i64 range; saturating at i64::MAX",
                            span=tok.span,
                            file=self.name,
                        )
                    )
                    return self.module.add(
                        Node("literal", Value.integer(I64_MAX), span=tok.span)
                    )
                return self.module.add(
                    Node("literal", Value.integer(tok.value), span=tok.span)
                )
            value = float(tok.value)
            if math.isinf(value):
                self.diagnostics.append(
                    error(
                        "E023",
                        "float literal out of range; saturating",
                        span=tok.span,
                        file=self.name,
                    )
                )
                value = sys.float_info.max
            return self.module.add(
                Node("literal", Value.floating(value), span=tok.span)
            )
        if tok.kind == T.BOOL:
            self.cur.next()
            return self.module.add(
                Node("literal", Value.boolean(bool(tok.value)), span=tok.span)
            )
        if tok.kind == T.NULL:
            self.cur.next()
            return self.module.add(Node("literal", Value.null(), span=tok.span))
        if tok.kind == T.PUNCT and tok.value == "(":
            self.cur.next()
            self.nesting += 1
            if self.nesting > 65:
                raise ParseFailure(
                    [
                        error(
                            "E004",
                            "expression nesting limit exceeded",
                            span=tok.span,
                            file=self.name,
                        )
                    ]
                )
            try:
                node = self._expression()
                self.cur.expect_punct(")")
            finally:
                self.nesting -= 1
            return node
        if tok.kind == T.PUNCT and tok.value == "[":
            return self._array()
        if tok.kind == T.PUNCT and tok.value == "{":
            return self._inline_table()
        if tok.kind == T.AT:
            self.cur.next()
            tag = self.cur.expect(T.IDENT).value
            if tag not in ("prompt", "policy", "template"):
                raise ParseFailure(
                    [
                        error(
                            "E014",
                            f"unknown semantic tag `@{tag}`",
                            span=tok.span,
                            file=self.name,
                        )
                    ]
                )
            return self._primary()
        if tok.kind == T.IDENT:
            name = tok.value
            if name == "env" and self.cur.at_punct(".", 1):
                return self._env()
            if name == "secret" and (
                self.cur.at_punct("(", 1) or self.cur.at_punct(".", 1)
            ):
                return self._secret()
            if name == "blob" and self.cur.at_punct("(", 1):
                return self._blob()
            if name in ("true", "false", "null"):
                pass
            self.cur.next()
            path = [name]
            while self.cur.at_punct(".") and self.cur.peek(1).kind == T.IDENT:
                self.cur.next()
                path.append(self.cur.next().value)
            return self.module.add(Node("ident", name=".".join(path), span=tok.span))
        raise ParseFailure(
            [error("E002", "expected an expression", span=tok.span, file=self.name)]
        )

    def _string_node(self, tok: T.Token) -> int:
        if not tok.parts:
            return self.module.add(Node("literal", Value.string(""), span=tok.span))
        if all(part.kind == "lit" for part in tok.parts):
            return self.module.add(
                Node(
                    "literal",
                    Value.string("".join(p.text for p in tok.parts)),
                    span=tok.span,
                )
            )
        parts: list[tuple[str, Any]] = []
        for part in tok.parts:
            if part.kind == "lit":
                parts.append(("lit", part.text))
            else:
                from .lexer import lex as _lex

                sub = _lex(part.text, self.name)
                self.diagnostics.extend(sub.diagnostics)
                saved = self.cur
                self.cur = Cursor(sub.tokens)
                try:
                    self.cur.skip_newlines()
                    expr = self._expression()
                finally:
                    self.cur = saved
                parts.append(("expr", expr))
        return self.module.add(Node("interpolate", parts=parts))

    def _array(self) -> int:
        self.nesting += 1
        if self.nesting > 65:
            self.nesting -= 1
            raise ParseFailure(
                [
                    error(
                        "E004",
                        "expression nesting limit exceeded",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        try:
            open_tok = self.cur.next()  # [
            items: list[int] = []
            while not self.cur.at_punct("]"):
                self.cur.skip_newlines()
                if self.cur.at_punct("]"):
                    break
                items.append(self._expression())
                self.cur.skip_newlines()
                if not self.cur.accept_punct(","):
                    self.cur.skip_newlines()
            self.cur.expect_punct("]")
            return self.module.add(Node("array", children=items, span=open_tok.span))
        finally:
            self.nesting -= 1

    def _inline_table(self) -> int:
        self.nesting += 1
        if self.nesting > 65:
            raise ParseFailure(
                [
                    error(
                        "E004",
                        "expression nesting limit exceeded",
                        span=self.cur.peek().span,
                        file=self.name,
                    )
                ]
            )
        try:
            return self._inline_table_body()
        finally:
            self.nesting -= 1

    def _inline_table_body(self) -> int:
        self.cur.expect_punct("{")
        builder = TBuilder()
        saved = self.current
        self.current = builder
        while not self.cur.at_punct("}"):
            self.cur.skip_newlines()
            if self.cur.at_punct("}"):
                break
            name = self.cur.expect(T.IDENT).value
            if not (self.cur.accept_punct("=") or self.cur.accept_punct(":")):
                raise ParseFailure(
                    [
                        error(
                            "E001",
                            "expected `=` in inline table",
                            span=self.cur.peek().span,
                            file=self.name,
                        )
                    ]
                )
            builder.fields[name] = self._expression()
            self.cur.skip_newlines()
            self.cur.accept_punct(",")
            self.cur.skip_newlines()
        self.cur.expect_punct("}")
        self.current = saved
        return self._materialize(builder, None, "")

    def _env(self) -> int:
        env_tok = self.cur.next()  # env
        self.cur.expect_punct(".")
        name_tok = self.cur.expect(T.IDENT)
        default = None
        if self.cur.at_op("?:"):
            self.cur.next()
            default = self._expression()
        return self.module.add(
            Node(
                "env",
                name=name_tok.value.upper(),
                children=[default] if default is not None else [],
                span=env_tok.span,
            )
        )

    def _secret(self) -> int:
        keyword = self.cur.next()  # secret
        if self.cur.accept_punct("."):
            self.cur.expect(T.IDENT, "env")
            self.cur.expect_punct("(")
            if self.cur.peek().kind != T.STRING:
                raise ParseFailure(
                    [
                        error(
                            "E003",
                            "secret.env requires a string literal",
                            span=self.cur.peek().span,
                            file=self.name,
                        )
                    ]
                )
            name_tok = self.cur.next()
            name = _string_value(name_tok, self)
            self.cur.expect_punct(")")
            env = self.module.add(Node("env", name=name.upper()))
            return self.module.add(Node("secret", children=[env], span=keyword.span))
        self.cur.expect_punct("(")
        locator = self._expression()
        self.cur.expect_punct(")")
        return self.module.add(Node("secret", children=[locator], span=keyword.span))

    def _blob(self) -> int:
        keyword = self.cur.next()  # blob
        self.cur.expect_punct("(")
        media = self._expression()
        self.cur.expect_punct(",")
        data = self._expression()
        self.cur.expect_punct(")")
        return self.module.add(
            Node("content_addressed", children=[media, data], span=keyword.span)
        )

    # -- materialize and resolve -----------------------------------------
    def _materialize(self, builder: TBuilder, scope: Scope | None, path: str) -> int:
        fields: dict[str, int] = {}
        table_scope = Scope(scope)
        node = Node("table", span=None)
        node_id = self.module.add(node)
        for name, value in builder.fields.items():
            if isinstance(value, TBuilder):
                fields[name] = self._materialize(value, table_scope, _join(path, name))
            else:
                fields[name] = value
        self.module.get(node_id).fields = fields
        table_scope.fields = fields
        if not hasattr(self.module, "_scope_of"):
            self.module._scope_of = {}
            self.module._self_of = {}
            self.module.ident_target = {}
        self.module._scope_of[node_id] = table_scope
        for name, value in builder.fields.items():
            if not isinstance(value, TBuilder):
                self._propagate_scope(value, table_scope, name)
        return node_id

    def _propagate_scope(self, node_id: int, scope: Scope, self_name: str) -> None:
        node = self.module.get(node_id)
        if node_id in self.module._scope_of:
            if node.kind == "table":
                table_scope = self.module._scope_of.get(node_id)
                if table_scope is not None and table_scope.parent is None:
                    table_scope.parent = scope
            return
        self.module._scope_of[node_id] = scope
        self.module._self_of[node_id] = (scope, self_name)
        if node.kind == "table":
            table_scope = self.module._scope_of.get(node_id)
            if table_scope is not None and table_scope.parent is None:
                table_scope.parent = scope
            return
        for child in node.children:
            self._propagate_scope(child, scope, self_name)
        for child in node.fields.values():
            self._propagate_scope(child, scope, self_name)
        if node.kind == "interpolate":
            for kind, part in node.parts:
                if kind == "expr" and isinstance(part, int):
                    self._propagate_scope(part, scope, self_name)

    def _attach_immutables(self) -> None:
        code_lines = sorted(
            {
                token.span.line
                for token in self.cur.tokens
                if token.span is not None
                and token.kind not in (T.NEWLINE, T.INDENT, T.DEDENT, T.EOF)
            }
        )
        flagged: set[int] = set()
        for comment in self.comments:
            if comment.trailing:
                continue
            text = comment.text.strip()
            if not text.startswith("@ai_immutable"):
                continue
            payload = text[len("@ai_immutable") :].strip().lstrip(":").strip()
            if payload not in ("", "true"):
                continue
            target = next((line for line in code_lines if line > comment.line), None)
            if target is not None:
                flagged.add(target)
        if flagged:
            for path, span in self.module.value_spans.items():
                if span.line in flagged:
                    self.module.immutable_paths.add(path)
        for comment in self.comments:
            if comment.trailing:
                continue
            text = comment.text.strip()
            if not text.startswith("@constraint:"):
                continue
            source = text[len("@constraint:") :].strip()
            target = next((line for line in code_lines if line > comment.line), None)
            if target is None or not source:
                continue
            for path, span in self.module.value_spans.items():
                if span.line == target:
                    self.module.constraints.append(
                        (path, source, Span(comment.line, 1, comment.line, 1))
                    )

    def _resolve_all(self) -> None:
        builders = getattr(self, "_profile_builders", {})
        for name, builder in builders.items():
            scope = self.module._scope_of.get(self.module.root)
            self.module.profiles[name] = self._materialize(
                builder, scope, f"profile.{name}"
            )
        root_scope = self.module._scope_of.get(self.module.root)
        if self.module.schema is not None and root_scope is not None:
            for _name, expr in self.module.schema.checks:
                self._propagate_scope(expr, root_scope, "")
        for node_id, node in enumerate(self.module.nodes):
            if node.kind != "ident":
                continue
            scope = self.module._scope_of.get(node_id)
            self_name = self.module._self_of.get(node_id, (None, ""))[1]
            target = self._resolve_ident(node, scope, self_name)
            if target is not None:
                self.module.ident_target[node_id] = target
            else:
                self.module.diagnostics.append(
                    warning(
                        "W001",
                        f"unresolved identifier `{node.name}`",
                        path=node.name,
                        span=node.span,
                        file=self.name,
                    )
                )

    def _resolve_ident(
        self, node: Node, scope: Scope | None, self_name: str = ""
    ) -> int | None:
        if scope is None:
            return None
        segments = (node.name or "").split(".")
        current = scope
        found: int | None = None
        first = segments[0]
        first_scope = True
        while current is not None and found is None:
            if first in current.fields and not (first_scope and first == self_name):
                found = current.fields[first]
            else:
                current = current.parent
            first_scope = False
        if found is None:
            found = self.module.bindings.get(first)
        if found is None:
            return None
        for segment in segments[1:]:
            child = self.module.get(found)
            if child.kind != "table" or segment not in child.fields:
                return None
            found = child.fields[segment]
        return found


def _join(path: str, name: str) -> str:
    return f"{path}.{name}" if path else name


def _string_value(tok: T.Token, parser: Parser) -> str:
    if tok.parts and all(part.kind == "lit" for part in tok.parts):
        return "".join(part.text for part in tok.parts)
    raise ParseFailure(
        [error("E001", "expected a string literal", span=tok.span, file=parser.name)]
    )


def _literal_string(module: Module, node_id: int) -> str | None:
    node = module.get(node_id)
    if node.kind == "literal" and node.value.kind == "string":
        return node.value.data
    return None


def _is_schema_version(text: str) -> bool:
    """`major.minor.patch`, all digits, like the shipped parser (E020)."""
    parts = text.split(".")
    return len(parts) == 3 and all(
        part and all("0" <= char <= "9" for char in part) for part in parts
    )


def parse(source: str, name: str = "<source>") -> Module:
    return Parser(source, name).parse()
