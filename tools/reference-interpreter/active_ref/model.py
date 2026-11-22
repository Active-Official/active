"""The spec's IR: values, nodes, schema types, and the module."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from .diag import Diagnostic, Span
from .units import Unit

# -- values ---------------------------------------------------------------


@dataclass
class Blob:
    hash: str
    size: int
    media_type: str
    tainted: bool
    payload: bytes | None = None


@dataclass
class Value:
    kind: str
    data: Any = None
    unit: Unit | None = None
    # P37-N2: a scalar computed from secret operands stays material for
    # every export path even when its evaluated kind is plain Bool/Int/
    # Float/String. Containers carry it on their leaves.
    material: bool = False
    # First authored span that produced this value (budget diagnostics).
    span: Span | None = None
    # For tables: the authored span of each field occurrence, so a shared
    # alias still reports the occurrence that crossed a budget.
    field_spans: dict[str, Span | None] | None = None

    @staticmethod
    def null() -> Value:
        return Value("null")

    @staticmethod
    def boolean(value: bool) -> Value:
        return Value("bool", value)

    @staticmethod
    def integer(value: int) -> Value:
        return Value("int", value)

    @staticmethod
    def floating(value: float) -> Value:
        return Value("float", value)

    @staticmethod
    def string(value: str) -> Value:
        return Value("string", value)

    @staticmethod
    def duration(ms: int) -> Value:
        return Value("duration", ms)

    @staticmethod
    def quantity(value: float, unit: Unit) -> Value:
        return Value("quantity", value, unit)

    @staticmethod
    def array(items: list[Value]) -> Value:
        return Value("array", items)

    @staticmethod
    def table(
        fields: dict[str, Value],
        field_spans: dict[str, Span | None] | None = None,
    ) -> Value:
        return Value("table", fields, field_spans=field_spans)

    @staticmethod
    def secret(text: str) -> Value:
        return Value("secret", text, material=True)

    @staticmethod
    def blob(blob: Blob) -> Value:
        return Value("blob", blob)


# -- schema ---------------------------------------------------------------


@dataclass
class SchemaType:
    kind: str  # a bare type name, Array, Table, Enum, OneOf
    element: SchemaType | None = None
    fields: dict[str, SchemaField] = field(default_factory=dict)
    closed: bool = False
    members: list[str] = field(default_factory=list)
    branches: list[SchemaType] = field(default_factory=list)


@dataclass
class SchemaField:
    typ: SchemaType
    args: dict[str, int] = field(default_factory=dict)  # arg name -> node id
    description: str | None = None
    deprecated: str | None = None  # None = absent; "" = bare true
    removed_in: str | None = None
    requires: str | None = None
    blob_media_type: str | None = None
    blob_max_size: int | None = None
    span: Span | None = None


@dataclass
class SchemaTable:
    fields: dict[str, SchemaField] = field(default_factory=dict)
    closed: bool = False
    checks: list[tuple[str, int]] = field(default_factory=list)


# -- nodes ----------------------------------------------------------------


@dataclass
class Node:
    kind: str
    value: Any = None
    children: list[int] = field(default_factory=list)
    fields: dict[str, int] = field(default_factory=dict)
    name: str | None = None
    op: str | None = None
    unit: Unit | None = None
    span: Span | None = None
    parts: list[tuple[str, Any]] = field(default_factory=list)
    # The include statement line that spliced this node (None for main).
    include_line: int | None = None


@dataclass
class IncludeStmt:
    path: str
    pin: str | None
    alias: str | None
    private: bool
    span: Span | None = None


@dataclass
class Patch:
    kind: str  # patch | remove | rename
    path: str
    value: int | None = None
    new_name: str | None = None
    span: Span | None = None
    # Include-relative application key: (include line, sequence) for an
    # included patch; None for a local patch (its own span line).
    order: tuple[int, int] | None = None


@dataclass
class Annotation:
    kind: str  # intent | constraint | ai_immutable
    payload: str
    target: int | None = None


@dataclass
class Module:
    source_name: str
    source: str
    nodes: list[Node] = field(default_factory=list)
    root: int = 0
    bindings: dict[str, int] = field(default_factory=dict)
    profiles: dict[str, int] = field(default_factory=dict)
    schema: SchemaTable | None = None
    schema_node: int | None = None
    includes: list[IncludeStmt] = field(default_factory=list)
    patches: list[Patch] = field(default_factory=list)
    annotations: list[Annotation] = field(default_factory=list)
    mode: str = "config"
    diagnostics: list[Diagnostic] = field(default_factory=list)
    healing: list[Diagnostic] = field(default_factory=list)
    source_style: str = "indent"
    # Dotted value path -> the authored token span, for validation spans.
    value_spans: dict[str, Span] = field(default_factory=dict)
    check_spans: dict[int, Span] = field(default_factory=dict)
    immutable_paths: set[str] = field(default_factory=set)
    constraints: list[tuple[str, str, Span | None]] = field(default_factory=list)
    schema_span: Span | None = None
    profile_spans: dict[str, Span] = field(default_factory=dict)
    internal_ids: set[int] = field(default_factory=set)

    def add(self, node: Node) -> int:
        self.nodes.append(node)
        return len(self.nodes) - 1

    def get(self, node_id: int) -> Node:
        return self.nodes[node_id]
