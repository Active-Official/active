"""Schema validation, defaults, closed tables, requires, checks (L-P37).

Implements the verdict-affecting rules from spec/validation.md and
schema-extensions.md. Repairs beyond string coercion and default fills
(the healer) are out of the oracle's first scope; affected cases are
gap-blocked rather than silently accepted.
"""

from __future__ import annotations

from .diag import Diagnostic, Span, error
from .eval import Evaluator
from .model import SchemaField, SchemaTable, Value

try:
    import regex as _regex
except ImportError:  # pragma: no cover
    import re as _regex


ERROR = "error"
WARNING = "warning"


class Validator:
    def __init__(self, evaluator: Evaluator, resilient: bool):
        self.ev = evaluator
        self.resilient = resilient
        self.diags = evaluator.diagnostics

    def validate(self, value: Value, schema: SchemaTable | None) -> Value:
        if schema is None:
            return value
        self.root = value
        result = self._table(value, schema, "")
        # A requires-triggered missing key and the generic missing-key
        # check both describe the same condition; the shipped engine
        # reports it once (the CLI's second line is a summary).
        seen: set[str | None] = set()
        kept: list[Diagnostic] = []
        for diagnostic in self.diags:
            if diagnostic.code == "T004":
                if diagnostic.path in seen:
                    continue
                seen.add(diagnostic.path)
            kept.append(diagnostic)
        self.diags[:] = kept
        return result

    def _lookup_root(self, path: str) -> Value | None:
        current = getattr(self, "root", None)
        for segment in path.split("."):
            if (
                current is None
                or current.kind != "table"
                or segment not in current.data
            ):
                return None
            current = current.data[segment]
        return current

    # -- diagnostics ------------------------------------------------------
    def _fail(
        self, code: str, path: str, message: str, span: Span | None = None
    ) -> None:
        if span is None:
            span = getattr(self, "_context_span", None)
        if span is None:
            span = getattr(self.ev.module, "value_spans", {}).get(path)
        self.diags.append(error(code, message, path=path or None, span=span))

    # -- tables -----------------------------------------------------------
    def _table(self, value: Value, schema: SchemaTable, path: str) -> Value:
        if value.kind != "table":
            self._fail("T002", path, f"expected Table, got {_kind(value)}")
            return value
        fields = value.data
        # Validate present keys in schema order.
        for name, field in schema.fields.items():
            child_path = _join(path, name)
            if name in fields:
                fields[name] = self._field(fields[name], field, child_path)
            else:
                if "default" in field.args:
                    if field.deprecated is not None and not self.resilient:
                        continue
                    if self.resilient:
                        default = self.ev.eval_node(field.args["default"])
                        default_span = self.ev.module.get(field.args["default"]).span
                        previous = getattr(self, "_context_span", None)
                        self._context_span = default_span
                        try:
                            fields[name] = self._field(default, field, child_path)
                        finally:
                            self._context_span = previous
                        self.diags.append(
                            Diagnostic(
                                "W005",
                                WARNING,
                                f"filled missing key '{child_path}' from schema default",
                                path=child_path or None,
                                span=field.span,
                            )
                        )
                    else:
                        self._fail(
                            "T004", child_path, "missing required key", field.span
                        )
                else:
                    self._fail("T004", child_path, "missing required key", field.span)
        if schema.closed:
            for key in fields:
                if key in schema.fields:
                    continue
                if not path and key in (
                    "active_mode",
                    "active_capabilities",
                    "active_profile",
                ):
                    continue
                self._fail(
                    "T005", _join(path, key), f"unexpected key '{key}' in closed table"
                )
        self._requires(fields, schema, path)
        return value

    def _requires(
        self, fields: dict[str, Value], schema: SchemaTable, path: str
    ) -> None:
        for name, field in schema.fields.items():
            if not field.requires:
                continue
            target = field.requires
            if "." in target:
                target_value = self._lookup_root(target)
                present = target_value is not None
                enabled = present and _enabled_value(target_value)
            else:
                if target not in schema.fields:
                    self._fail(
                        "T003",
                        _join(path, name),
                        f"requires target '{target}' is not a schema path",
                        field.span,
                    )
                    continue
                trigger = schema.fields[target]
                present = target in fields
                enabled = present and _enabled(fields[target], trigger)
            if enabled and name not in fields:
                self._fail(
                    "T004",
                    _join(path, name),
                    f"missing required key '{name}' (triggered by '{target}')",
                    field.span,
                )
            if name in fields and not enabled:
                self._fail(
                    "T010",
                    _join(path, name),
                    f"'{name}' present while its trigger '{target}' is disabled",
                )

    # -- fields -----------------------------------------------------------
    def _field(self, value: Value, field: SchemaField, path: str) -> Value:
        typ = field.typ
        kind = typ.kind
        if kind == "Any":
            return value
        if kind == "OneOf":
            return self._one_of(value, field, path)
        if kind == "Secret":
            if value.kind != "secret":
                self._fail("T002", path, f"expected Secret, got {_kind(value)}")
            return value
        if value.kind == "secret":
            self._fail("T002", path, f"expected {kind}, got secret")
            return value
        if kind == "Int":
            if value.kind != "int":
                self._fail("T002", path, f"expected Int, got {_kind(value)}")
                return value
            self._int_bounds(value, field, path)
        elif kind == "Float":
            if value.kind not in ("float", "int"):
                self._fail("T002", path, f"expected Float, got {_kind(value)}")
                return value
            self._float_bounds(value, field, path)
        elif kind == "String":
            if value.kind != "string":
                self._fail("T002", path, f"expected String, got {_kind(value)}")
                return value
            self._string(value, field, path)
        elif kind == "Bool":
            if value.kind != "bool":
                self._fail("T002", path, f"expected Bool, got {_kind(value)}")
        elif kind == "Null":
            if value.kind != "null":
                self._fail("T002", path, f"expected Null, got {_kind(value)}")
        elif kind == "Duration":
            if value.kind == "duration":
                self._duration_bounds(value, field, path)
            elif value.kind == "int" and value.data >= 0:
                pass
            else:
                self._fail("T002", path, f"expected Duration, got {_kind(value)}")
        elif kind == "Bytes":
            if value.kind != "int":
                self._fail("T002", path, f"expected Bytes, got {_kind(value)}")
            else:
                self._bytes_bounds(value, field, path)
        elif kind == "Enum":
            if value.kind != "string" or value.data not in typ.members:
                self._fail("T003", path, "value is not an enum member")
        elif kind == "Array":
            if value.kind != "array":
                self._fail("T002", path, f"expected Array, got {_kind(value)}")
            elif typ.element:
                element_field = SchemaField(typ.element)
                for index, item in enumerate(value.data):
                    value.data[index] = self._field(
                        item, element_field, f"{path}[{index}]"
                    )
        elif kind == "Table":
            table = SchemaTable(fields=typ.fields, closed=typ.closed)
            value = self._table(value, table, path)
        elif kind == "Blob":
            self._blob(value, field, path)
        else:
            self._fail("T002", path, f"unknown schema type {kind}")
        return value

    def _one_of(self, value: Value, field: SchemaField, path: str) -> Value:
        default = self._arg(field, "default")
        if default is not None:
            ok_any = False
            for branch in field.typ.branches:
                _, ok = self._branch_result(default, branch, path)
                if ok:
                    ok_any = True
                    break
            if not ok_any:
                default_node = self.ev.module.get(field.args["default"])
                self._fail(
                    "T002",
                    path,
                    "OneOf default matches no branch",
                    default_node.span,
                )
        for branch in field.typ.branches:
            result, ok = self._branch_result(value, branch, path)
            if ok:
                return result
        self._fail("T002", path, "value matches no OneOf branch")
        return value

    def _branch_result(self, value: Value, field: SchemaField, path: str):
        before = len(self.diags)
        copy = _copy_value(value)
        result = self._field(copy, field, path)
        new_errors = [d for d in self.diags[before:] if d.severity == ERROR]
        if new_errors:
            del self.diags[before:]
            return None, False
        del self.diags[before:]
        return result, True

    # -- bounds -----------------------------------------------------------
    def _arg(self, field: SchemaField, name: str) -> Value | None:
        node = field.args.get(name)
        if node is None:
            return None
        return self.ev.eval_node(node)

    def _int_bounds(self, value: Value, field: SchemaField, path: str) -> None:
        low = self._arg(field, "min")
        high = self._arg(field, "max")
        if low and low.kind in ("int", "float") and value.data < low.data:
            self._fail("T003", path, "below minimum")
        if high and high.kind in ("int", "float") and value.data > high.data:
            self._fail("T003", path, "above maximum")

    def _float_bounds(self, value: Value, field: SchemaField, path: str) -> None:
        low = self._arg(field, "min")
        high = self._arg(field, "max")
        if low and low.kind in ("int", "float") and float(value.data) < float(low.data):
            self._fail("T003", path, "below minimum")
        if (
            high
            and high.kind in ("int", "float")
            and float(value.data) > float(high.data)
        ):
            self._fail("T003", path, "above maximum")

    def _duration_bounds(self, value: Value, field: SchemaField, path: str) -> None:
        low = self._arg(field, "min")
        high = self._arg(field, "max")
        if low and low.kind in ("duration", "int") and value.data < low.data:
            self._fail("T003", path, "below minimum")
        if high and high.kind in ("duration", "int") and value.data > high.data:
            self._fail("T003", path, "above maximum")

    def _bytes_bounds(self, value: Value, field: SchemaField, path: str) -> None:
        low = self._arg(field, "min")
        high = self._arg(field, "max")
        if low and low.kind == "int" and value.data < low.data:
            self._fail("T003", path, "below minimum")
        if high and high.kind == "int" and value.data > high.data:
            self._fail("T003", path, "above maximum")

    def _string(self, value: Value, field: SchemaField, path: str) -> None:
        low = self._arg(field, "min_len")
        high = self._arg(field, "max_len")
        length = len(value.data)
        if low and low.kind == "int" and length < max(0, low.data):
            self._fail("T003", path, "too short")
        if high and high.kind == "int" and length > max(0, high.data):
            self._fail("T003", path, "too long")
        pattern = self._arg(field, "pattern")
        if pattern and pattern.kind == "string":
            if not _pattern_dialect_ok(pattern.data):
                self._fail("T003", path, "invalid pattern in schema")
            else:
                try:
                    if not _regex.search(pattern.data, value.data):
                        self._fail("T003", path, "does not match pattern")
                except _regex.error:
                    self._fail("T003", path, "invalid pattern in schema")
        fmt = self._arg(field, "format")
        if fmt and fmt.kind == "string" and not _format_ok(fmt.data, value.data):
            self._fail("T003", path, f"does not match format '{fmt.data}'")

    def _blob(self, value: Value, field: SchemaField, path: str) -> None:
        if value.kind != "blob":
            self._fail("T002", path, f"expected Blob, got {_kind(value)}")
            return
        blob = value.data
        if field.blob_max_size is not None and blob.size > field.blob_max_size:
            self._fail("T002", path, "blob exceeds max_size")
        if (
            field.blob_media_type is not None
            and blob.media_type != field.blob_media_type
        ):
            self._fail("T004", path, "blob media type mismatch")


def _enabled(value: Value, field: SchemaField) -> bool:
    if field.typ.kind == "Null":
        return False
    if value.kind == "secret" or value.kind == "blob":
        return True
    if value.kind in ("bool",):
        return value.data
    if value.kind in ("int",):
        return value.data != 0
    if value.kind == "float":
        return value.data != 0.0
    if value.kind == "string":
        return value.data != ""
    if value.kind == "duration":
        return value.data != 0
    if value.kind == "quantity":
        return value.data != 0.0
    if value.kind == "array":
        return len(value.data) > 0
    if value.kind == "table":
        return len(value.data) > 0
    return False


def _enabled_value(value: Value) -> bool:
    if value.kind in ("secret", "blob"):
        return True
    if value.kind == "bool":
        return value.data
    if value.kind == "int":
        return value.data != 0
    if value.kind == "float":
        return value.data != 0.0
    if value.kind == "string":
        return value.data != ""
    if value.kind == "duration":
        return value.data != 0
    if value.kind == "quantity":
        return value.data != 0.0
    if value.kind == "array":
        return len(value.data) > 0
    if value.kind == "table":
        return len(value.data) > 0
    return False


def _kind(value: Value) -> str:
    return {
        "null": "Null",
        "bool": "Bool",
        "int": "Int",
        "float": "Float",
        "string": "String",
        "duration": "Duration",
        "quantity": "quantity",
        "array": "Array",
        "table": "Table",
        "secret": "Secret",
        "blob": "Blob",
    }.get(value.kind, value.kind)


def _join(path: str, name: str) -> str:
    return f"{path}.{name}" if path else name


def _copy_value(value: Value) -> Value:
    if value.kind == "table":
        return Value.table({k: _copy_value(v) for k, v in value.data.items()})
    if value.kind == "array":
        return Value.array([_copy_value(v) for v in value.data])
    return Value(value.kind, value.data, value.unit)


def _pattern_dialect_ok(pattern: str) -> bool:
    """The Rust `regex` dialect (G24): no backreferences, no look-around.

    The PyPI `regex` engine accepts both, so a schema that Rust refuses
    with T003 must be refused here before the match is attempted.
    """
    forbidden = (
        r"\\[1-9]",
        r"\\k<",
        r"\(\?P=",
        r"\(\?=",
        r"\(\?!",
        r"\(\?<=",
        r"\(\?<!",
    )
    return all(_regex.search(regex, pattern) is None for regex in forbidden)


def _format_ok(name: str, text: str) -> bool:
    patterns = {
        "email": r"^[^@\s]+@[^@\s]+\.[^@\s]+$",
        "uri": r"^[A-Za-z][A-Za-z0-9+.-]*:[^\s]*$",
        "uuid": r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
        "semver": r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$",
    }
    pattern = patterns.get(name)
    if pattern is None:
        return False
    return _regex.search(pattern, text) is not None
