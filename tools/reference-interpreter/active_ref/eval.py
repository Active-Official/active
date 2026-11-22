"""Evaluator: values, units, names, profiles, patches, budgets (L-P37).

Implements the shipped semantics the approved design and Bud's gap
decisions specify. The oracle is written from the spec; composite
equality deliberately matches shipped behavior (always false, P37-N1).
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Any

from blake3 import blake3

from .diag import Diagnostic, Span
from .model import Blob, Module, Node, Value
from .units import (
    I64_MAX,
    I64_MIN,
    U64_MAX,
    affine_or_log,
    canonicalize,
    format_amount,
    same_dimension,
)


class EvalError(Exception):
    def __init__(
        self,
        code: str,
        message: str,
        span: Span | None = None,
        path: str | None = None,
    ):
        super().__init__(message)
        self.code = code
        self.message = message
        self.span = span
        self.path = path


def type_name(value: Value) -> str:
    if value.kind == "quantity":
        return f"quantity:{value.unit.suffix}"
    return {
        "null": "null",
        "bool": "bool",
        "int": "int",
        "float": "float",
        "string": "string",
        "duration": "duration",
        "array": "array",
        "table": "table",
        "secret": "secret",
        "blob": "blob",
    }.get(value.kind, value.kind)


def _clamp_i64(n: int) -> int:
    return max(I64_MIN, min(I64_MAX, n))


def _clamp_u64(n: int) -> int:
    return max(0, min(U64_MAX, n))


def _float_result(value: float) -> Value:
    if not math.isfinite(value):
        raise EvalError("T001", "non-finite float result")
    return Value.floating(value)


def value_eq(a: Value, b: Value) -> bool:
    # P37-N1: composites/secrets/blobs are never equal in the shipped
    # engine; the spec's structural claim stands and is gap-blocked.
    if a.kind != b.kind:
        return False
    if a.kind == "null":
        return True
    if a.kind == "bool":
        return a.data == b.data
    if a.kind == "int":
        return a.data == b.data
    if a.kind == "float":
        return a.data == b.data
    if a.kind == "string":
        return a.data == b.data
    if a.kind == "duration":
        return a.data == b.data
    if a.kind == "quantity":
        return a.data == b.data and a.unit == b.unit
    return False


@dataclass
class Budget:
    max_nodes: int = 100_000
    max_depth: int = 256
    max_array_len: int = 100_000
    max_keys: int = 100_000
    max_value_size: int = 10_000_000
    max_total_value_size: int = 100_000_000
    max_wall_ms: int = 100

    def unlimited(self) -> Budget:
        return Budget(U64_MAX, U64_MAX, U64_MAX, U64_MAX, U64_MAX, U64_MAX, 0)

    @staticmethod
    def trusted() -> Budget:
        # Mirror `active-core::Budget::trusted()` (MC engine), wall limit
        # disabled as the MC dump sets UNLIMITED.
        return Budget(
            10_000_000,
            2_048,
            10_000_000,
            10_000_000,
            1_000_000_000,
            10_000_000_000,
            0,
        )


@dataclass
class Engine:
    resilient: bool = True
    profile_override: str | None = None
    budget: Budget = field(default_factory=Budget)
    env: dict[str, str | None] = field(default_factory=dict)
    pure: bool = False
    materialise_blobs: bool = False
    reader_version: str | None = None
    blob_salt: bytes | None = None
    mode_override: str | None = None


@dataclass
class Outcome:
    value: Value | None
    diagnostics: list[Diagnostic]
    seeds: set[str] = field(default_factory=set)


class Evaluator:
    def __init__(self, module: Module, engine: Engine):
        self.module = module
        self.engine = engine
        self.values: dict[int, Value] = {}
        self.visiting: set[int] = set()
        self.diagnostics: list[Diagnostic] = list(module.diagnostics)
        self.seeds: set[str] = set()
        self.nodes_visited = 0
        self.total_size = 0
        self.constants: dict[str, Value] = {}

    # -- entry ------------------------------------------------------------
    def run(self) -> Outcome:
        self._pure_check()
        # G02 phase order (matches the shipped engine): reader floors
        # refuse before the graph walk; the cycle check precedes the
        # static shape budgets (G16).
        self._reader_checks()
        self._cycle_check()
        self._static_budgets()
        root = self.module.root
        value = self.eval_node(root)
        # Every profile evaluates, selected or not (spec §3.5): an error in
        # an unselected branch still refuses the document.
        for profile_node in self.module.profiles.values():
            self.eval_node(profile_node)
        self._mode(value)
        if self.module.mode == "data":
            self._progressive_warnings()
        value = self._apply_profile(value)
        value = self._apply_patches(value)
        value = self._validate(value)
        value = self._checks(value)
        value = self._constraints(value)
        self._envelope_check(value)
        # Accountant after defaults and checks: fills are real growth.
        self._check_result(value)
        # Export-side seeds (exact-match strings/keys and substring text).
        self.seeds.update(self._secret_literals())
        return Outcome(value, self.diagnostics, self.seeds)

    def _pure_check(self) -> None:
        if not self.engine.pure:
            return
        if self.engine.profile_override is not None:
            raise EvalError("E047", "pure evaluation refused a profile override")
        if any(stmt.pin is None for stmt in self.module.includes):
            raise EvalError("E047", "pure evaluation refused an unpinned include")
        for node in self.module.nodes:
            if node.kind == "env":
                raise EvalError(
                    "E047",
                    f"pure evaluation refused an env read: '{node.name}'",
                    node.span,
                )

    def _reader_checks(self) -> None:
        reader = self.engine.reader_version or "1.4.0"
        floor = getattr(self.module, "schema_min_reader", None)
        if floor and not _version_at_least(reader, floor):
            raise EvalError("T007", f"document requires reader {floor}")
        if self.module.schema is not None:
            for name, field in self.module.schema.fields.items():
                if field.removed_in and _version_at_least(reader, field.removed_in):
                    self.diagnostics.append(
                        Diagnostic(
                            "T008",
                            "error",
                            f"field '{name}' was removed in {field.removed_in}",
                            path=f"{name}.{name}",
                            span=field.span,
                        )
                    )

    def _checks(self, value: Value) -> Value:
        if self.module.schema is None:
            return value
        # Checks see the FINAL tree: profile/patch/default results replace
        # the cached root-key values before the predicates run.
        if value.kind == "table":
            root_fields = self.module.get(self.module.root).fields
            for key, final in value.data.items():
                node_id = root_fields.get(key)
                if node_id is not None:
                    self.values[node_id] = final
        for name, expr in self.module.schema.checks:
            try:
                result = self.eval_node(expr)
            except EvalError:
                self.diagnostics.append(
                    Diagnostic(
                        "T009",
                        "error",
                        f"check '{name}' could not be evaluated",
                        span=self.module.check_spans.get(expr),
                    )
                )
                continue
            if result.kind != "bool":
                self.diagnostics.append(
                    Diagnostic(
                        "T009",
                        "error",
                        f"check '{name}' must evaluate to a boolean",
                        span=self.module.check_spans.get(expr),
                    )
                )
            elif not result.data:
                self.diagnostics.append(
                    Diagnostic(
                        "T009",
                        "error",
                        f"check '{name}' failed",
                        span=self.module.check_spans.get(expr),
                    )
                )
        return value

    def _payload_material(self, value: Value) -> bool:
        seeds = self._secret_literals()

        def walk(current: Value) -> bool:
            if current.kind == "secret" or current.material:
                return True
            if current.kind == "blob":
                return current.data.tainted or self._material_tainted(
                    current.data.media_type, seeds
                )
            if current.kind == "string":
                return self._material_tainted(current.data, seeds)
            if current.kind == "table":
                for key, item in current.data.items():
                    if self._material_tainted(key, seeds) or walk(item):
                        return True
                return False
            if current.kind == "array":
                return any(walk(item) for item in current.data)
            return False

        return walk(value)

    def _span(self, path: str):
        return self.module.value_spans.get(path)

    def _constraints(self, value: Value) -> Value:
        if not self.module.constraints:
            return value
        from .parser import parse

        for path, source, span in self.module.constraints:
            target = value
            for segment in path.split("."):
                if target.kind != "table" or segment not in target.data:
                    target = None
                    break
                target = target.data[segment]
            try:
                synthetic = parse("__constraint = " + source + "\n", "__constraint")
                evaluator = Evaluator(synthetic, self.engine)
                evaluator.constants = (
                    {name: item for name, item in value.data.items()}
                    if value.kind == "table"
                    else {}
                )
                if target is not None:
                    evaluator.constants["value"] = target
                node_id = synthetic.get(synthetic.root).fields["__constraint"]
                result = evaluator.eval_node(node_id)
            except Exception:  # noqa: BLE001 - any predicate failure is T006
                result = None
            if result is None or result.kind != "bool" or not result.data:
                from .export import MARKER, _redact_text

                seeds = sorted(
                    {seed for seed in self._secret_literals() if seed},
                    key=lambda s: len(s.encode("utf-8")),
                    reverse=True,
                )
                rendered = _redact_text(source, seeds)
                if any(
                    len(seed.encode("utf-8")) >= 4 and seed in source for seed in seeds
                ):
                    rendered = MARKER
                self.diagnostics.append(
                    Diagnostic(
                        "T006",
                        "error",
                        f"constraint violation at '{path}': {rendered} does not hold",
                        path=path,
                        span=span,
                    )
                )
        return value

    def _bound_hash(
        self, payload: Value | None, tainted: bool, capability: str | None
    ) -> str:
        import hashlib
        import json

        budget = self.engine.budget
        defaults = Budget()
        pick = lambda used, default: default if used == 0 else used
        object_value = {
            "active.envelope": 2,
            "tainted": tainted,
            "mode": self.module.mode,
            "budget": {
                "nodes": pick(budget.max_nodes, defaults.max_nodes),
                "depth": pick(budget.max_depth, defaults.max_depth),
                "array_len": pick(budget.max_array_len, defaults.max_array_len),
                "keys": pick(budget.max_keys, defaults.max_keys),
                "wall_ms": pick(budget.max_wall_ms, defaults.max_wall_ms),
                "value_size": pick(budget.max_value_size, defaults.max_value_size),
                "total_size": pick(
                    budget.max_total_value_size, defaults.max_total_value_size
                ),
            },
            "value": _hash_projection(payload, self.seeds)
            if payload is not None
            else None,
        }
        if capability is not None:
            object_value["capability"] = capability
        version = getattr(self.module, "schema_version", None)
        if version is not None:
            object_value["schema_version"] = version
        profile = self.engine.profile_override
        if profile is None:
            root = self.module.get(self.module.root)
            selector = (
                root.fields.get("active_profile") if root.kind == "table" else None
            )
            if selector is not None:
                selected = self.values.get(selector)
                if selected is not None and selected.kind == "string":
                    profile = selected.data
        if profile is not None:
            object_value["profile"] = profile
        canonical = json.dumps(
            object_value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        )
        return "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()

    def _envelope_check(self, value: Value) -> None:
        # The envelope contract is a Data-mode surface (spec/mode.md), and
        # every refusal is fatal in the shipped engine (a failed envelope
        # has no tree), so these raise instead of collecting.
        if self.module.mode != "data" or value.kind != "table":
            return
        caps = value.data.get("active_capabilities")
        if caps is not None:
            if caps.kind != "array" or any(item.kind != "string" for item in caps.data):
                raise EvalError(
                    "E025",
                    "active_capabilities must be strings",
                    self._span("active_capabilities"),
                    path="active_capabilities",
                )
            for item in caps.data:
                if item.data not in _KNOWN_CAPABILITIES:
                    raise EvalError(
                        "E034",
                        f"capability '{item.data}' is not offered by this host",
                        path="active_capabilities",
                    )
        envelope = value.data.get("envelope")
        if envelope is None:
            return
        if envelope.kind != "table":
            raise EvalError(
                "E033",
                "envelope must be a table",
                self._span("envelope"),
                path="envelope",
            )
        version = envelope.data.get("version")
        if version is None or version.kind != "string":
            raise EvalError(
                "E033",
                "envelope version is malformed",
                self._span("envelope.version"),
                path="envelope.version",
            )
        if _semver(version.data) is None:
            raise EvalError(
                "E033",
                "envelope version is malformed",
                self._span("envelope.version"),
                path="envelope.version",
            )
        major = version.data.split(".")[0]
        if major not in ("1", "2"):
            raise EvalError(
                "E033",
                f"unknown envelope major '{major}'",
                self._span("envelope.version"),
                path="envelope.version",
            )
        if major == "2":
            receipt = envelope.data.get("receipt")
            if receipt is None or receipt.kind != "table":
                raise EvalError(
                    "E042",
                    "envelope v2 requires a receipt",
                    path="envelope.receipt",
                )
            outcome = receipt.data.get("outcome")
            if (
                outcome is None
                or outcome.kind != "string"
                or outcome.data
                not in (
                    "clean",
                    "failed",
                )
            ):
                raise EvalError(
                    "E042",
                    "receipt.outcome must be clean or failed",
                    path="envelope.receipt.outcome",
                )
            if outcome.data == "failed":
                failure = receipt.data.get("failure_class")
                if (
                    failure is None
                    or failure.kind != "string"
                    or failure.data not in ("parse", "budget", "validation", "other")
                ):
                    raise EvalError(
                        "E042",
                        "receipt.failure_class is required on a failed outcome",
                        path="envelope.receipt.failure_class",
                    )
            else:
                mode = receipt.data.get("mode")
                if (
                    mode is None
                    or mode.kind != "string"
                    or mode.data
                    not in (
                        "config",
                        "data",
                    )
                ):
                    raise EvalError(
                        "E042",
                        "a clean receipt must carry its mode",
                        path="envelope.receipt.mode",
                    )
                budget_table = receipt.data.get("budget")
                limits = (
                    "nodes",
                    "depth",
                    "array_len",
                    "keys",
                    "wall_ms",
                    "value_size",
                    "total_size",
                )
                if (
                    budget_table is None
                    or budget_table.kind != "table"
                    or not all(
                        key in budget_table.data
                        and budget_table.data[key].kind == "int"
                        and budget_table.data[key].data > 0
                        for key in limits
                    )
                ):
                    raise EvalError(
                        "E042",
                        "a clean receipt must carry its seven positive budget limits",
                        path="envelope.receipt.budget",
                    )
            sender = envelope.data.get("sender")
            if sender is not None:
                if sender.kind != "table":
                    raise EvalError(
                        "E043",
                        "envelope.sender must be a table",
                        path="envelope.sender",
                    )
                sender_id = sender.data.get("id")
                if (
                    sender_id is None
                    or sender_id.kind != "string"
                    or not sender_id.data
                ):
                    raise EvalError(
                        "E043",
                        "envelope.sender.id must be a non-empty string",
                        path="envelope.sender.id",
                    )
                key = sender.data.get("key")
                signature = sender.data.get("signature")
                if (key is None) != (signature is None):
                    raise EvalError(
                        "E043",
                        "sender key and signature are given together",
                        path="envelope.sender",
                    )
                if signature is not None and (
                    signature.kind != "string"
                    or len(signature.data) != 128
                    or any(
                        char not in "0123456789abcdefABCDEF" for char in signature.data
                    )
                ):
                    raise EvalError(
                        "E043",
                        "sender signature must be 128 hex characters",
                        path="envelope.sender.signature",
                    )
        tainted = envelope.data.get("tainted")
        if tainted is None:
            raise EvalError(
                "E035",
                "envelope is missing its tainted claim",
                path="envelope.tainted",
            )
        if tainted.kind != "bool":
            raise EvalError(
                "E035",
                "envelope tainted claim is not a boolean",
                path="envelope.tainted",
            )
        payload = envelope.data.get("payload")
        if payload is not None:
            actual = self._payload_material(payload)
            if tainted.data != actual:
                raise EvalError(
                    "E035",
                    "envelope taint claim does not match its payload",
                    path="envelope.tainted",
                )
        if major == "2":
            result = envelope.data.get("result")
            receipt_result = (
                envelope.data["receipt"].data.get("result")
                if envelope.data["receipt"].kind == "table"
                else None
            )
            if result is None or result.kind != "string":
                raise EvalError(
                    "E044",
                    "a clean v2 envelope requires its bound result",
                    path="envelope.result",
                )
            capability = envelope.data.get("capability")
            capability_text = (
                capability.data
                if capability is not None and capability.kind == "string"
                else None
            )
            expected = self._bound_hash(payload, bool(tainted.data), capability_text)
            if (
                result.data != expected
                or receipt_result is None
                or receipt_result.kind != "string"
                or receipt_result.data != expected
            ):
                raise EvalError(
                    "E044",
                    "envelope result does not match the bound payload",
                    path="envelope.result",
                )
        if (
            any(node.kind == "env" for node in self.module.nodes)
            or self.module.includes
            or self.module.profiles
        ):
            raise EvalError(
                "E036",
                "envelope document is not self-contained (env read)",
                next(
                    (node.span for node in self.module.nodes if node.kind == "env"),
                    None,
                ),
            )

    def _mode(self, value: Value) -> None:
        if value.kind != "table" or "active_mode" not in value.data:
            self.module.mode = "config"
            return
        marker = value.data["active_mode"]
        if marker.kind != "string" or marker.data not in ("config", "data"):
            raise EvalError(
                "E024",
                "invalid active_mode marker",
                self._span("active_mode"),
                path="active_mode",
            )
        self.module.mode = marker.data

    # -- budgets ----------------------------------------------------------
    def _cycle_check(self) -> None:
        roots: list[int] = [self.module.root]
        roots.extend(self.module.bindings.values())
        roots.extend(self.module.profiles.values())
        roots.extend(p.value for p in self.module.patches if p.value is not None)
        if self.module.schema is not None:
            for field in self.module.schema.fields.values():
                roots.extend(field.args.values())
            roots.extend(expr for _name, expr in self.module.schema.checks)
        color: dict[int, int] = {}
        for start in roots:
            stack: list[tuple[int, bool]] = [(start, False)]
            while stack:
                node_id, exiting = stack.pop()
                state = color.get(node_id, 0)
                if exiting:
                    color[node_id] = 2
                    continue
                if state == 1:
                    raise EvalError("S001", "cycle detected")
                if state == 2:
                    continue
                color[node_id] = 1
                stack.append((node_id, True))
                node = self.module.get(node_id)
                adjacent = list(node.children) + list(node.fields.values())
                if node.kind == "interpolate":
                    adjacent.extend(
                        part
                        for kind, part in node.parts
                        if kind == "expr" and isinstance(part, int)
                    )
                target = getattr(self.module, "ident_target", {}).get(node_id)
                if target is not None:
                    adjacent.append(target)
                stack.extend((child, False) for child in adjacent)

    def _topo_order(self) -> list[int]:
        order: list[int] = []
        color: dict[int, int] = {}
        for start in range(len(self.module.nodes)):
            stack: list[tuple[int, bool]] = [(start, False)]
            while stack:
                node_id, exiting = stack.pop()
                state = color.get(node_id, 0)
                if exiting:
                    color[node_id] = 2
                    order.append(node_id)
                    continue
                if state == 1:
                    raise EvalError("S001", "cycle detected")
                if state == 2:
                    continue
                color[node_id] = 1
                stack.append((node_id, True))
                node = self.module.get(node_id)
                adjacent = list(node.children) + list(node.fields.values())
                if node.kind == "interpolate":
                    adjacent.extend(
                        part
                        for kind, part in node.parts
                        if kind == "expr" and isinstance(part, int)
                    )
                target = getattr(self.module, "ident_target", {}).get(node_id)
                if target is not None:
                    adjacent.append(target)
                stack.extend((child, False) for child in adjacent)
        return order

    def _static_budgets(self) -> None:
        budget = self.engine.budget
        if len(self.module.nodes) > budget.max_nodes:
            raise EvalError(
                "X001",
                f"node budget exceeded ({len(self.module.nodes)} > {budget.max_nodes})",
            )
        keys = 0
        for node in self.module.nodes:
            if node.kind == "table":
                keys += len(node.fields)
            elif node.kind == "array" and len(node.children) > budget.max_array_len:
                raise EvalError(
                    "X004",
                    f"array length budget exceeded ({len(node.children)} > {budget.max_array_len})",
                    node.span,
                )
            if keys > budget.max_keys:
                raise EvalError(
                    "X005", f"key budget exceeded ({keys} > {budget.max_keys})"
                )
        bound_map = self._bound_values()
        bounds = [bound_map[i] for i in range(len(self.module.nodes))]
        depths = self._depth_values()
        # Per node (topological order in the shipped DP): value size is
        # checked before depth; the cumulative total is last (spec §2b).
        for node_id in self._topo_order():
            bound = bounds[node_id]
            if budget.max_value_size and bound > budget.max_value_size:
                raise EvalError(
                    "X006",
                    f"value size budget exceeded ({bound} > {budget.max_value_size})",
                    self.module.get(node_id).span,
                )
            if budget.max_depth and depths[node_id] > budget.max_depth:
                raise EvalError(
                    "X003",
                    f"depth budget exceeded ({depths[node_id]} > {budget.max_depth})",
                    self.module.get(node_id).span,
                )
        total = sum(bounds)
        if budget.max_total_value_size and total > budget.max_total_value_size:
            raise EvalError(
                "X006",
                f"total value size budget exceeded ({total} > {budget.max_total_value_size})",
            )

    def _max_depth(self) -> int:
        return max(self._depth_values().values(), default=0)

    def _deps(self, node_id: int, node: Node) -> list[int]:
        if node.kind == "ident":
            target = getattr(self.module, "ident_target", {}).get(node_id)
            return [target] if target is not None else []
        if node.kind in ("secret", "env"):
            # Material locators and defaults are not materialized copies.
            return []
        if node.kind == "table":
            return list(node.fields.values())
        if node.kind == "array":
            return list(node.children)
        if node.kind == "interpolate":
            return [part for kind, part in node.parts if isinstance(part, int)]
        return list(node.children)

    def _depth_values(self) -> dict[int, int]:
        """Value depth over the shipped DP arms (G02/§2b), never recursion.

        Only tables, arrays, interpolations, and blob constructions add a
        nesting level; a reference copies its target's depth. Expression
        nodes are parser-capped and do not deepen the value.
        """
        depths: dict[int, int] = {}
        for start in range(len(self.module.nodes)):
            stack: list[tuple[int, bool]] = [(start, False)]
            while stack:
                node_id, expanded = stack.pop()
                if node_id in depths:
                    continue
                node = self.module.get(node_id)
                if node.kind in ("table", "array", "content_addressed", "interpolate"):
                    deps = self._deps(node_id, node)
                    if expanded:
                        depths[node_id] = 1 + max(
                            (depths.get(child, 0) for child in deps), default=0
                        )
                    else:
                        stack.append((node_id, True))
                        stack.extend((d, False) for d in deps if d not in depths)
                elif node.kind in ("ident", "semantic"):
                    deps = self._deps(node_id, node)
                    if expanded:
                        depths[node_id] = depths.get(deps[0], 0) if deps else 0
                    else:
                        stack.append((node_id, True))
                        stack.extend((d, False) for d in deps if d not in depths)
                else:
                    depths[node_id] = 0
        return depths

    def _bound_values(self) -> dict[int, int]:
        """Iterative, memoized DP size bound with u64 saturation."""
        bounds: dict[int, int] = {}
        for start in range(len(self.module.nodes)):
            stack: list[tuple[int, bool]] = [(start, False)]
            while stack:
                current, expanded = stack.pop()
                if current in bounds:
                    continue
                node = self.module.get(current)
                deps = self._deps(current, node)
                if expanded:
                    bounds[current] = min(U64_MAX, self._bound_from(node, deps, bounds))
                else:
                    stack.append((current, True))
                    stack.extend((d, False) for d in deps if d not in bounds)
        return bounds

    def _bound(self, node_id: int) -> int:
        return self._bound_values().get(node_id, 1)

    def _bound_from(self, node: Node, deps: list[int], bounds: dict[int, int]) -> int:
        if node.kind == "literal":
            if node.value.kind == "string":
                return 1 + len(node.value.data.encode("utf-8"))
            return 1
        if node.kind == "quantity":
            return 1
        if node.kind == "ident":
            return bounds.get(deps[0], 1) if deps else 1
        if node.kind == "semantic":
            return bounds.get(deps[0], 1) if deps else 1
        if node.kind == "interpolate":
            total = 1
            for kind, part in node.parts:
                if kind == "lit":
                    total += len(part.encode("utf-8"))
                elif isinstance(part, int):
                    total += bounds.get(part, 1)
            return total
        if node.kind in ("table", "array", "content_addressed"):
            total = 1
            for dep in deps:
                total += bounds.get(dep, 1)
            return total
        if node.kind == "binary":
            total = 1
            for dep in deps:
                total += bounds.get(dep, 1)
            return total
        if node.kind == "unary":
            return 1 + (bounds.get(deps[0], 1) if deps else 0)
        # Secret/env lookups and any other leaf cost one unit.
        return 1

    def _value_children(self, value: Value) -> list[Value]:
        if value.kind == "array":
            return list(value.data)
        if value.kind == "table":
            return list(value.data.values())
        return []

    def _actual_metrics(
        self, root: Value
    ) -> tuple[int, int, int, Span | None, Span | None, dict[int, int]]:
        """(total, max node size, depth, max-size span, depth span, sizes)."""
        sizes: dict[int, int] = {}
        depths: dict[int, int] = {}
        max_size = 0
        max_span: Span | None = None
        max_depth = 0
        depth_span: Span | None = None
        stack: list[tuple[Value, bool]] = [(root, False)]
        while stack:
            value, expanded = stack.pop()
            key = id(value)
            if key in sizes:
                continue
            children = self._value_children(value)
            if expanded:
                if value.kind in ("table", "array"):
                    size = 1 + sum(
                        min(U64_MAX, sizes.get(id(child), 1)) for child in children
                    )
                    depth = 1 + max(
                        (depths.get(id(child), 0) for child in children), default=0
                    )
                elif value.kind == "string":
                    size = 1 + len(value.data.encode("utf-8"))
                    depth = 0
                else:
                    size = 1
                    depth = 0
                sizes[key] = min(U64_MAX, size)
                depths[key] = depth
                if sizes[key] >= max_size:
                    max_size = sizes[key]
                    max_span = value.span
                if depth >= max_depth:
                    max_depth = depth
                    depth_span = value.span
            else:
                stack.append((value, True))
                stack.extend(
                    (child, False) for child in children if id(child) not in sizes
                )
        total = 0
        stack = [root]
        while stack:
            value = stack.pop()
            total = min(U64_MAX, total + sizes.get(id(value), 1))
            stack.extend(reversed(self._value_children(value)))
        return total, max_size, max_depth, max_span, depth_span, sizes

    def _check_result(self, value: Value) -> None:
        budget = self.engine.budget
        (
            total,
            max_size,
            depth,
            max_span,
            depth_span,
            _sizes,
        ) = self._actual_metrics(value)
        if budget.max_value_size and max_size > budget.max_value_size:
            raise EvalError(
                "X006",
                f"value size budget exceeded ({max_size} > {budget.max_value_size})",
                max_span,
            )
        if budget.max_depth and depth > budget.max_depth:
            raise EvalError(
                "X003",
                f"depth budget exceeded ({depth} > {budget.max_depth})",
                depth_span,
            )
        if budget.max_total_value_size and total > budget.max_total_value_size:
            # Source-order accumulation: the span where the running total
            # crosses the limit, matching the shipped accountant.
            running = 0
            crossing: Span | None = None
            stack: list[tuple[Value, Span | None]] = [(value, value.span)]
            while stack:
                current, occurrence_span = stack.pop()
                running = min(U64_MAX, running + _charge_size(current))
                if running > budget.max_total_value_size and crossing is None:
                    crossing = occurrence_span
                if current.kind == "table" and isinstance(current.data, dict):
                    children = [
                        (
                            current.data[name],
                            (current.field_spans or {}).get(name)
                            or current.data[name].span,
                        )
                        for name in current.data
                    ]
                else:
                    children = [
                        (child, child.span) for child in self._value_children(current)
                    ]
                stack.extend(reversed(children))
            raise EvalError(
                "X006",
                f"total value size budget exceeded ({total} > {budget.max_total_value_size})",
                crossing if crossing is not None else value.span,
            )

    def _progressive_warnings(self) -> None:
        budget = self.engine.budget
        if self.module.mode != "data":
            return
        bound_map = self._bound_values()
        bounds = [bound_map[i] for i in range(len(self.module.nodes))]
        max_node_size = max(bounds, default=0)
        total = sum(bounds)
        keys = sum(
            1 for node in self.module.nodes if node.kind == "table" for _ in node.fields
        )
        max_arr = max(
            (len(node.children) for node in self.module.nodes if node.kind == "array"),
            default=0,
        )
        # G21 order: Nodes, ValueSize, Depth, TotalSize, ArrayLength, Keys.
        for label, used, limit in [
            ("Nodes", len(self.module.nodes), budget.max_nodes),
            ("ValueSize", max_node_size, budget.max_value_size),
            ("Depth", self._max_depth(), budget.max_depth),
            ("TotalSize", total, budget.max_total_value_size),
            ("ArrayLength", max_arr, budget.max_array_len),
            ("Keys", keys, budget.max_keys),
        ]:
            if limit and used * 5 >= limit * 4:
                self.diagnostics.append(
                    Diagnostic(
                        "W013",
                        "warning",
                        f"approaching {label} ({used} of limit {limit})",
                    )
                )

    # -- evaluation -------------------------------------------------------
    def eval_node(self, node_id: int) -> Value:
        if node_id in self.values:
            return self.values[node_id]
        if node_id in self.visiting:
            raise EvalError("S001", "cycle detected")
        self.visiting.add(node_id)
        node = self.module.get(node_id)
        if node.kind == "ident" and node.name in self.constants:
            return self.constants[node.name]
        if node.kind == "ident":
            target = getattr(self.module, "ident_target", {}).get(node_id)
            if target is None:
                return Value.null()
            value = self.eval_node(target)
            self.values[node_id] = value
            return value
        self.nodes_visited += 1
        try:
            value = self._eval(node)
        finally:
            self.visiting.discard(node_id)
        if value.span is None and node.span is not None:
            value.span = node.span
        self.values[node_id] = value
        return value

    def _eval(self, node: Node) -> Value:
        kind = node.kind
        if kind == "literal":
            return node.value
        if kind == "table":
            values: dict[str, Value] = {}
            # G02: evaluation follows the shipped graph walk. For
            # independent siblings that order is reverse source order
            # (petgraph's topological tie-break); it decides which error
            # is reported when several bindings fail independently.
            spans = {
                name: self.module.get(child).span for name, child in node.fields.items()
            }
            for name, child in reversed(list(node.fields.items())):
                values[name] = self.eval_node(child)
            return Value.table(
                {name: values[name] for name in node.fields}, field_spans=spans
            )
        if kind == "array":
            return Value.array([self.eval_node(c) for c in node.children])
        if kind == "binary":
            left = self.eval_node(node.children[0])
            right = self.eval_node(node.children[1])
            return _binary(node.op, left, right, node.span)
        if kind == "unary":
            child = self.module.get(node.children[0])
            if node.op == "-" and child.kind == "quantity":
                return self._negated_quantity(
                    child, node.span, self.module.get(node.children[0]).span
                )
            value = self.eval_node(node.children[0])
            return _unary(node.op, value, node.span)
        if kind == "env":
            name = (node.name or "").upper()
            if name in self.engine.env:
                raw = self.engine.env[name]
                if raw is not None:
                    return Value.string(raw)
                if node.children:
                    return self.eval_node(node.children[0])
                return Value.null()
            if node.children:
                return self.eval_node(node.children[0])
            return Value.null()
        if kind == "secret":
            locator = (
                self.eval_node(node.children[0]) if node.children else Value.null()
            )
            if locator.kind == "string":
                return Value.secret(locator.data)
            if locator.kind == "secret":
                return locator
            # A non-string locator is an empty secret; it is still material.
            return Value.secret("")
        if kind == "interpolate":
            out: list[str] = []
            material = False
            for part_kind, part in node.parts:
                if part_kind == "lit":
                    out.append(part)
                else:
                    value = self.eval_node(part)
                    if value.kind == "secret":
                        raise EvalError(
                            "S004", "secret used in interpolation", node.span
                        )
                    material = material or value.material
                    out.append(self._interpolate_value(value))
            result = Value.string("".join(out))
            result.material = material
            return result
        if kind == "quantity":
            canon = canonicalize(float(node.value), node.unit)
            if canon.kind == "duration":
                return Value.duration(int(canon.value))
            if canon.kind == "int":
                return Value.integer(int(canon.value))
            if canon.unit.suffix == "K" and canon.value < 0.0:
                raise EvalError("T001", "value below absolute zero", node.span)
            return Value.quantity(canon.value, canon.unit)
        if kind == "content_addressed":
            media = self.eval_node(node.children[0])
            data = self.eval_node(node.children[1])
            return self._eval_blob(media, data, node.span)
        if kind == "ident":
            # Unresolved identifiers become null with W001 during parse.
            return Value.null()
        raise EvalError("T001", f"unknown node kind {kind}", node.span)

    def _negated_quantity(
        self, node: Node, op_span: Span | None, literal_span: Span | None
    ) -> Value:
        # A negated Cel/Fahr literal is affine folding: the shipped span is
        # the literal. Absolute K (and durations) are arithmetic: the
        # operator is the span.
        span = (
            literal_span
            if node.unit.kind == "quantity" and node.unit.suffix in ("Cel", "Fahr")
            else op_span
        )
        amount = -float(node.value)
        unit = node.unit
        if unit.kind == "duration":
            if amount != 0:
                raise EvalError("T001", "negated duration", span)
            return Value.duration(0)
        if unit.kind == "bytes":
            return Value.integer(_clamp_i64(round(amount * unit.factor)))
        canon = canonicalize(amount, unit)
        if canon.unit.suffix == "K" and canon.value < 0.0:
            raise EvalError(
                "T001", "negation produced a value below absolute zero", span
            )
        return Value.quantity(canon.value, canon.unit)

    def _secret_literals(self) -> list[str]:
        out: list[str] = []
        for node_id, node in enumerate(self.module.nodes):
            if node.kind != "secret":
                continue
            try:
                material = self.eval_node(node_id)
            except EvalError:
                continue
            if material.kind == "secret" and material.data:
                out.append(material.data)
        for value in self.values.values():
            if value.kind == "secret" and value.data:
                out.append(value.data)
        return out

    def _material_tainted(self, text: str, seeds: list[str]) -> bool:
        for seed in seeds:
            if text == seed:
                return True
            if len(seed.encode("utf-8")) >= 4 and seed in text:
                return True
        return False

    def _eval_blob(self, media: Value, data: Value, span: Span | None) -> Value:
        if media.kind not in ("string", "secret"):
            raise EvalError("B001", "blob media type must be a string", span)
        if data.kind not in ("string", "secret"):
            raise EvalError("B001", "blob data must be a string", span)
        seeds = self._secret_literals()
        media_text = media.data if media.kind == "string" else ""
        data_text = data.data if data.kind == "string" else ""
        tainted = (
            media.material
            or data.material
            or media.kind == "secret"
            or data.kind == "secret"
            or self._material_tainted(media_text, seeds)
            or self._material_tainted(data_text, seeds)
        )
        payload = data_text.encode("utf-8")
        media_bytes = media_text.encode("utf-8")
        hasher = blake3()
        if tainted:
            salt = _document_salt(self.module)
            hasher.update(b"active-blob/v2\x00")
            hasher.update(len(salt).to_bytes(8, "little"))
            hasher.update(salt)
        else:
            hasher.update(b"active-blob/v1\x00")
        hasher.update(len(media_bytes).to_bytes(8, "little"))
        hasher.update(media_bytes)
        hasher.update(len(payload).to_bytes(8, "little"))
        hasher.update(payload)
        return Value.blob(
            Blob(
                "blake3:" + hasher.hexdigest(), len(payload), media_text, tainted, None
            )
        )

    def _interpolate_value(self, value: Value) -> str:
        if value.kind in ("string",):
            return value.data
        if value.kind == "int":
            return str(value.data)
        if value.kind == "float":
            if not math.isfinite(value.data):
                return (
                    "nan"
                    if math.isnan(value.data)
                    else ("inf" if value.data > 0 else "-inf")
                )
            if value.data == int(value.data):
                return str(int(value.data))
            from .export import float_text

            return float_text(value.data)
        if value.kind == "bool":
            return "true" if value.data else "false"
        if value.kind == "null":
            return "null"
        if value.kind == "duration":
            return f"{value.data}ms"
        if value.kind == "quantity":
            return format_amount(value.data, value.unit)
        if value.kind == "secret":
            raise EvalError("S004", "secret used in interpolation")
        raise EvalError("T001", f"cannot interpolate {type_name(value)}")

    # -- profiles and patches ---------------------------------------------
    def _apply_profile(self, root: Value) -> Value:
        if root.kind != "table":
            return root
        selection: str | None = None
        if self.engine.profile_override is not None:
            selection = self.engine.profile_override
        elif "active_profile" in root.data:
            chosen = root.data["active_profile"]
            if chosen.kind == "string":
                selection = chosen.data
            else:
                self.diagnostics.append(
                    Diagnostic(
                        "W002",
                        "warning",
                        "active_profile is not a string; no profile merges",
                    )
                )
                return root
        if selection is None:
            return root
        if selection not in self.module.profiles:
            self.diagnostics.append(
                Diagnostic(
                    "W002",
                    "warning",
                    f"profile `{selection}` is not defined; no profile merges",
                )
            )
            return root
        overlay = self.eval_node(self.module.profiles[selection])
        if overlay.kind != "table":
            return root
        material_paths = _material_paths(root)
        merged = _merge_profile(root, overlay)
        if material_paths:
            _mark_material_paths(merged, material_paths)
        return merged

    def _apply_patches(self, root: Value) -> Value:
        if root.kind != "table":
            return root
        value = root
        charged = 0
        budget = self.engine.budget
        metadata = (
            "active_mode",
            "active_capabilities",
            "active_profile",
            "schema",
            "envelope",
        )
        for patch in self.module.patches:
            segments = _parse_path(patch.path)
            if segments and segments[0] in metadata:
                raise EvalError(
                    "E019",
                    f"patch cannot rewrite document metadata `{segments[0]}`",
                    patch.span,
                )
            if patch.kind == "rename" and any(
                marker in (patch.new_name or "") for marker in (".", "[", "]")
            ):
                raise EvalError(
                    "E019",
                    "rename target must be a single key name",
                    patch.span,
                )
            if (
                patch.kind == "rename"
                and len(segments) == 1
                and (patch.new_name or "") in metadata
            ):
                raise EvalError(
                    "E019",
                    f"patch cannot create document metadata `{patch.new_name}`",
                    patch.span,
                )
            for immutable in self.module.immutable_paths:
                if _paths_overlap(segments, immutable):
                    raise EvalError(
                        "E019",
                        f"patch target '{'.'.join(str(s) for s in segments)}' overlaps ai_immutable '{immutable}'",
                        patch.span,
                    )
            if patch.kind == "patch":
                staged = self.eval_node(patch.value)
                staged_size = self._actual_metrics(staged)[0]
                if budget.max_value_size and staged_size > budget.max_value_size:
                    raise EvalError(
                        "X006",
                        f"value size budget exceeded ({staged_size} > {budget.max_value_size})",
                        patch.span,
                    )
                charged = min(U64_MAX, charged + staged_size + len(segments))
                if (
                    budget.max_total_value_size
                    and charged > budget.max_total_value_size
                ):
                    raise EvalError(
                        "X006",
                        f"total value size budget exceeded ({charged} > {budget.max_total_value_size})",
                        patch.span,
                    )
                value = _set_path(value, segments, staged, patch.span)
            elif patch.kind == "remove":
                charged = min(U64_MAX, charged + 1 + len(segments))
                if (
                    budget.max_total_value_size
                    and charged > budget.max_total_value_size
                ):
                    raise EvalError(
                        "X006",
                        f"total value size budget exceeded ({charged} > {budget.max_total_value_size})",
                        patch.span,
                    )
                value = _remove_path(value, segments, self.diagnostics, patch.span)
            elif patch.kind == "rename":
                charged = min(U64_MAX, charged + 1 + len(segments))
                if (
                    budget.max_total_value_size
                    and charged > budget.max_total_value_size
                ):
                    raise EvalError(
                        "X006",
                        f"total value size budget exceeded ({charged} > {budget.max_total_value_size})",
                        patch.span,
                    )
                value = _rename_path(
                    value, segments, patch.new_name or "", self.diagnostics, patch.span
                )
        return value

    def _validate(self, value: Value) -> Value:
        # Local import avoids a module cycle (validate imports eval).
        from .validate import Validator

        if self.module.schema is None:
            return value
        return Validator(self, self.engine.resilient).validate(
            value, self.module.schema
        )


# -- operators ------------------------------------------------------------


def _binary(op: str, left: Value, right: Value, span: Span | None) -> Value:
    result = _binary_op(op, left, right, span)
    if left.material or right.material:
        result.material = True
    return result


def _binary_op(op: str, left: Value, right: Value, span: Span | None) -> Value:
    if op in ("==", "!="):
        equal = value_eq(left, right)
        return Value.boolean(equal if op == "==" else not equal)
    if op in ("&&", "||"):
        if left.kind != "bool" or right.kind != "bool":
            raise _type_error(op, left, right, span)
        return Value.boolean(
            (left.data and right.data) if op == "&&" else (left.data or right.data)
        )
    if op in ("<", "<=", ">", ">="):
        cmp = _compare(op, left, right, span)
        return Value.boolean(cmp)
    if left.kind == "int" and right.kind == "int":
        return _int_arith(op, left.data, right.data, span)
    if left.kind in ("int", "float") and right.kind in ("int", "float"):
        a = float(left.data)
        b = float(right.data)
        if op == "+":
            return _float_result(a + b)
        if op == "-":
            return _float_result(a - b)
        if op == "*":
            return _float_result(a * b)
        if op == "/":
            if b == 0:
                raise EvalError("T001", "division by zero", span)
            return _float_result(a / b)
    if left.kind in ("int", "float") and right.kind == "duration" and op == "/":
        if right.data == 0:
            raise EvalError("T001", "division by zero", span)
        return _float_result(float(left.data) / (float(right.data) / 1000.0))
    if left.kind == "duration":
        return _duration_arith(op, left, right, span)
    if left.kind == "quantity":
        return _quantity_arith(op, left, right, span)
    if left.kind == "int" and right.kind == "duration" and op in ("+", "-"):
        raise _type_error(op, left, right, span)
    raise _type_error(op, left, right, span)


def _int_arith(op: str, a: int, b: int, span: Span | None) -> Value:
    if op == "+":
        return Value.integer(_clamp_i64(a + b))
    if op == "-":
        return Value.integer(_clamp_i64(a - b))
    if op == "*":
        return Value.integer(_clamp_i64(a * b))
    if op == "/":
        if b == 0:
            raise EvalError("T001", "division by zero", span)
        if a == I64_MIN and b == -1:
            return Value.integer(I64_MAX)
        quotient = abs(a) // abs(b)
        return Value.integer(quotient if (a < 0) == (b < 0) else -quotient)
    raise EvalError("T001", f"unsupported operator {op}")


def _duration_arith(op: str, left: Value, right: Value, span: Span | None) -> Value:
    a = int(left.data)
    if op in ("+", "-") and right.kind == "duration":
        b = int(right.data)
        result = a + b if op == "+" else a - b
        if result < 0:
            raise EvalError("T001", "negative duration result", span)
        return Value.duration(_clamp_u64(result))
    if op == "*" and right.kind in ("int", "float"):
        result = a * float(right.data)
        if result < 0:
            raise EvalError("T001", "negative duration result", span)
        return Value.duration(_clamp_u64(round(result)))
    if op == "/" and right.kind in ("int", "float"):
        if right.data == 0:
            raise EvalError("T001", "division by zero", span)
        result = a / float(right.data)
        if result < 0:
            raise EvalError("T001", "negative duration result", span)
        return Value.duration(_clamp_u64(round(result)))
    if op == "/" and right.kind == "duration":
        if right.data == 0:
            raise EvalError("T001", "division by zero", span)
        return Value.integer(int(a / int(right.data)))
    raise _type_error(op, left, right, span)


def _quantity_arith(op: str, left: Value, right: Value, span: Span | None) -> Value:
    if affine_or_log(left.unit) and op not in ("<", "<=", ">", ">=", "==", "!="):
        raise EvalError("T001", "affine/log quantities have no arithmetic", span)
    if op in ("+", "-") and right.kind == "quantity":
        if not same_dimension(left.unit, right.unit):
            raise _type_error(op, left, right, span)
        b = right.data
        result = left.data + b if op == "+" else left.data - b
        return Value.quantity(result, left.unit)
    if op == "*" and right.kind in ("int", "float"):
        return Value.quantity(float(left.data) * float(right.data), left.unit)
    if op == "/" and right.kind in ("int", "float"):
        if right.data == 0:
            raise EvalError("T001", "division by zero", span)
        return Value.quantity(float(left.data) / float(right.data), left.unit)
    if op == "/" and right.kind == "quantity":
        if not same_dimension(left.unit, right.unit):
            raise _type_error(op, left, right, span)
        if right.data == 0:
            raise EvalError("T001", "division by zero", span)
        return _float_result(float(left.data) / float(right.data))
    raise _type_error(op, left, right, span)


def _compare(op: str, left: Value, right: Value, span: Span | None) -> bool:
    if left.kind in ("int", "float") and right.kind in ("int", "float"):
        a = float(left.data)
        b = float(right.data)
    elif (
        left.kind == "duration"
        and right.kind == "duration"
        or left.kind == "quantity"
        and right.kind == "quantity"
        and same_dimension(left.unit, right.unit)
    ):
        a, b = float(left.data), float(right.data)
    else:
        raise _type_error(op, left, right, span)
    if op == "<":
        return a < b
    if op == "<=":
        return a <= b
    if op == ">":
        return a > b
    return a >= b


def _unary(op: str, value: Value, span: Span | None) -> Value:
    result = _unary_op(op, value, span)
    if value.material:
        result.material = True
    return result


def _unary_op(op: str, value: Value, span: Span | None) -> Value:
    if op == "!":
        if value.kind != "bool":
            raise EvalError("T001", f"unsupported unary ! on {type_name(value)}", span)
        return Value.boolean(not value.data)
    if op == "-":
        if value.kind == "int":
            return Value.integer(_clamp_i64(-value.data))
        if value.kind == "float":
            return _float_result(-value.data)
        if value.kind == "duration":
            if value.data == 0:
                return value
            raise EvalError("T001", "negated duration", span)
        if value.kind == "quantity":
            if affine_or_log(value.unit):
                raise EvalError(
                    "T001", "affine/log quantities have no arithmetic", span
                )
            return Value.quantity(-value.data, value.unit)
        raise EvalError("T001", f"unsupported unary - on {type_name(value)}", span)
    raise EvalError("T001", f"unknown unary operator {op}")


def _type_error(op: str, left: Value, right: Value, span: Span | None) -> EvalError:
    return EvalError(
        "T001",
        f"unsupported binary op {op} on {type_name(left)} and {type_name(right)}",
        span,
    )


# -- profile merge / patches ---------------------------------------------


def _merge_profile(base: Value, overlay: Value) -> Value:
    if base.kind != "table" or overlay.kind != "table":
        return overlay
    out = dict(base.data)
    for key, value in overlay.data.items():
        if key in out and out[key].kind == "table" and value.kind == "table":
            out[key] = _merge_profile(out[key], value)
        else:
            out[key] = value
    return Value.table(out)


def _paths_overlap(left: list[Any], right: str) -> bool:
    try:
        other = _parse_path(right)
    except EvalError:
        # An immutable path that the patch grammar cannot express (a
        # non-ASCII key, for example) still overlaps any ancestor write:
        # fail closed by comparing its raw segments.
        raw = right.split(".") if right else []
        text = [str(segment) for segment in left]
        return text[: len(raw)] == raw or raw[: len(text)] == text
    return left[: len(other)] == other or other[: len(left)] == left


def _parse_path(path: str) -> list[Any]:
    """Strict `segment { '[' digits ']' }` path, matching the parser."""
    if not path:
        raise EvalError("E019", "patch path is empty")
    segments: list[Any] = []
    for part in path.split("."):
        if not part or not (part[0].isalpha() or part[0] == "_"):
            raise EvalError("E019", f"invalid patch path segment `{part}`")
        cursor = 0
        while cursor < len(part) and (part[cursor].isalnum() or part[cursor] == "_"):
            cursor += 1
        name = part[:cursor]
        if not all(char.isascii() for char in name):
            raise EvalError("E019", f"invalid patch path segment `{part}`")
        segments.append(name)
        while cursor < len(part):
            if part[cursor] != "[":
                raise EvalError("E019", f"invalid patch path segment `{part}`")
            close = part.find("]", cursor)
            if close < 0:
                raise EvalError("E019", f"unclosed patch path index in `{part}`")
            digits = part[cursor + 1 : close]
            if not digits.isdigit():
                raise EvalError("E019", "patch path index is not an integer")
            segments.append(int(digits))
            cursor = close + 1
    return segments


def _set_path(
    value: Value, segments: list[Any], new: Value, span: Span | None
) -> Value:
    if not segments:
        return new
    head, rest = segments[0], segments[1:]
    if isinstance(head, int):
        if value.kind != "array" or head < 0 or head >= len(value.data):
            raise EvalError("E019", "patch path out of bounds", span)
        items = list(value.data)
        items[head] = _set_path(items[head], rest, new, span)
        return Value.array(items)
    if value.kind != "table":
        raise EvalError("E019", "patch descends through a non-container", span)
    fields = dict(value.data)
    if rest:
        child = fields.get(head, Value.table({}))
        fields[head] = _set_path(child, rest, new, span)
    else:
        fields[head] = new
    return Value.table(fields)


def _remove_path(
    value: Value, segments: list[Any], diags: list[Diagnostic], span: Span | None
) -> Value:
    if not segments:
        return value
    head, rest = segments[0], segments[1:]
    if isinstance(head, int):
        if value.kind != "array" or head < 0 or head >= len(value.data):
            diags.append(
                Diagnostic("W004", "warning", "remove target missing", span=span)
            )
            return value
        items = list(value.data)
        if rest:
            items[head] = _remove_path(items[head], rest, diags, span)
        else:
            items.pop(head)
        return Value.array(items)
    if value.kind != "table":
        diags.append(Diagnostic("W004", "warning", "remove target missing", span=span))
        return value
    fields = dict(value.data)
    if head not in fields:
        diags.append(
            Diagnostic("W004", "warning", f"remove target `{head}` missing", span=span)
        )
        return value
    if rest:
        fields[head] = _remove_path(fields[head], rest, diags, span)
    else:
        del fields[head]
    return Value.table(fields)


def _rename_path(
    value: Value,
    segments: list[Any],
    new_name: str,
    diags: list[Diagnostic],
    span: Span | None,
) -> Value:
    if not segments:
        raise EvalError("E019", "rename needs a path", span)
    head, rest = segments[0], segments[1:]
    if isinstance(head, int):
        if value.kind != "array" or head < 0 or head >= len(value.data):
            raise EvalError("E019", "rename path out of bounds", span)
        items = list(value.data)
        items[head] = _rename_path(items[head], rest, new_name, diags, span)
        return Value.array(items)
    if value.kind != "table":
        raise EvalError("E019", "rename descends through a non-container", span)
    fields = dict(value.data)
    if head not in fields:
        diags.append(
            Diagnostic("W004", "warning", f"rename source `{head}` missing", span=span)
        )
        return value
    if rest:
        fields[head] = _rename_path(fields[head], rest, new_name, diags, span)
        return Value.table(fields)
    if new_name in fields:
        raise EvalError("E019", f"rename target `{new_name}` already exists", span)
    renamed: dict[str, Value] = {}
    for key, item in fields.items():
        renamed[new_name if key == head else key] = item
    return Value.table(renamed)


# -- blobs ----------------------------------------------------------------


def _semver(text: str) -> tuple[int, int, int] | None:
    """Strict `major.minor.patch` with u64 parts, like the shipped gate.

    An unparseable version fails the comparison closed: the caller has
    already rejected malformed metadata with E020, so this mirrors the
    Rust `version_at_least` defense-in-depth rule.
    """
    parts = text.split(".")
    if len(parts) != 3:
        return None
    out: list[int] = []
    for part in parts:
        if not part or not part.isdigit():
            return None
        value = int(part)
        if value > 0xFFFF_FFFF_FFFF_FFFF:
            return None
        out.append(value)
    return out[0], out[1], out[2]


def _version_at_least(have: str, need: str) -> bool:
    h = _semver(have)
    n = _semver(need)
    return h is not None and n is not None and h >= n


_KNOWN_CAPABILITIES = {
    "check",
    "eval",
    "convert",
    "lock",
    "audit",
    "explain",
    "generate",
    "fmt",
    "inspect",
    "graph",
    "repair",
    "doc",
    "serve",
}


def _charge_size(value: Value) -> int:
    if value.kind == "string":
        return 1 + len(value.data.encode("utf-8"))
    return 1


def _hash_projection(value: Value | None, seeds) -> object:
    from .export import MARKER, to_json_value

    if value is None:
        return None
    if value.kind == "secret" or (
        value.material and value.kind not in ("table", "array", "blob")
    ):
        return {"$active.secret": MARKER}
    if value.kind == "table":
        return {key: _hash_projection(item, seeds) for key, item in value.data.items()}
    if value.kind == "array":
        return [_hash_projection(item, seeds) for item in value.data]
    if value.kind == "blob":
        return {
            "$active.blob": {
                "hash": value.data.hash,
                "size": value.data.size,
                "media_type": value.data.media_type,
                "tainted": value.data.tainted,
            }
        }
    return to_json_value(value, seeds=seeds)


def _dotted(prefix: str, key: str) -> str:
    return f"{prefix}.{key}" if prefix else key


def _material_paths(root: Value, prefix: str = "") -> set[str]:
    found: set[str] = set()
    stack: list[tuple[Value, str]] = [(root, prefix)]
    while stack:
        value, path = stack.pop()
        if (
            value.kind in ("secret", "string")
            and value.material
            or value.kind == "blob"
            and value.data.tainted
        ):
            found.add(path)
        elif value.kind == "table":
            for key, item in value.data.items():
                stack.append((item, _dotted(path, key)))
        elif value.kind == "array":
            for item in value.data:
                stack.append((item, path))
    found.discard("")
    return found


def _mark_material_paths(root: Value, paths: set[str], prefix: str = "") -> None:
    if prefix in paths:
        _mark_material_tree(root)
        return
    if root.kind == "table":
        for key, item in root.data.items():
            _mark_material_paths(item, paths, _dotted(prefix, key))
    elif root.kind == "array":
        for item in root.data:
            _mark_material_paths(item, paths, prefix)


def _mark_material_tree(value: Value) -> None:
    if value.kind in ("table", "array"):
        children = value.data.values() if value.kind == "table" else value.data
        for item in children:
            _mark_material_tree(item)
    else:
        value.material = True


def _contains_secret(value: Value) -> bool:
    if value.kind == "secret" or value.material:
        return True
    if value.kind == "blob":
        return bool(value.data.tainted)
    if value.kind == "array":
        return any(_contains_secret(item) for item in value.data)
    if value.kind == "table":
        return any(_contains_secret(item) for item in value.data.values())
    return False


def _document_salt(module: Module) -> bytes:
    """Mirror the shipped arena projection in creation order.

    Main nodes keep parser creation order (interpolation/expression spans
    are not reliable ordering keys); the schema contributes a `?` per
    field plus `ST` at its source position; a profile table sits after
    its own fields; included nodes arrive after main nodes; the root
    table is last.
    """
    profile_ids = set(module.profiles.values())
    items: list[tuple[str, object]] = []
    for index, node in enumerate(module.nodes):
        if index == module.root or index in profile_ids or index in module.internal_ids:
            continue
        items.append(("node", (index, node)))
    # Schema: `?` per field, then ST, at the schema's source position.
    if module.schema is not None and module.schema_span is not None:
        position = 0
        for offset, (_kind, item) in enumerate(items):
            index, node = item
            if node.span is not None and node.span.line > module.schema_span.line:
                position = offset
                break
            position = offset + 1
        pseudo: list[tuple[str, object]] = [
            ("field", None) for _field in module.schema.fields
        ]
        pseudo.append(("schema", list(module.schema.fields)))
        items[position:position] = pseudo
    # Profile tables: after their last direct field.
    for node_id in module.profiles.values():
        table = module.get(node_id)
        positions = []
        for child in table.fields.values():
            for offset, (kind, item) in enumerate(items):
                if kind == "node" and item[0] == child:
                    positions.append(offset)
        insert_at = max(positions) + 1 if positions else 0
        items.insert(insert_at, ("node", (node_id, table)))
    if module.root is not None:
        items.append(("node", (module.root, module.get(module.root))))
    hasher = blake3()
    hasher.update(b"active-doc-salt/v1")
    for index, (kind, item) in enumerate(items):
        hasher.update(index.to_bytes(8, "little"))
        if kind == "field":
            hasher.update(b"?")
        elif kind == "schema":
            hasher.update(b"ST")
            for key in item:
                hasher.update(key.encode("utf-8"))
                hasher.update(b"\x00")
        else:
            node_index, node = item
            _salt_node(hasher, node, module, node_index)
    return hasher.digest()


def _salt_node(hasher, node, module, index) -> None:
    kind = node.kind
    if kind == "literal":
        hasher.update(b"L")
        _salt_scalar(hasher, node.value)
    elif kind == "quantity":
        hasher.update(b"Q")
        hasher.update(node.unit.suffix.encode("utf-8"))
    elif kind == "table":
        hasher.update(b"T")
        for key in node.fields:
            hasher.update(key.encode("utf-8"))
            hasher.update(b"\x00")
    elif kind == "array":
        hasher.update(b"A")
        hasher.update(len(node.children).to_bytes(8, "little"))
    elif kind == "secret":
        hasher.update(b"S")
    elif kind == "content_addressed":
        hasher.update(b"B")
    elif kind == "ident":
        target = getattr(module, "ident_target", {}).get(index)
        if target is not None:
            hasher.update(b"R")
        else:
            hasher.update(b"I")
            hasher.update((node.name or "").encode("utf-8"))
    elif kind == "env":
        hasher.update(b"E")
        hasher.update((node.name or "").encode("utf-8"))
    elif kind == "interpolate":
        hasher.update(b"P")
        hasher.update(len(node.parts).to_bytes(8, "little"))
    elif kind == "binary":
        hasher.update(b"O")
    elif kind == "unary":
        hasher.update(b"U")
    else:
        hasher.update(b"?")


def _salt_scalar(hasher, value: Value) -> None:
    if value.kind == "null":
        hasher.update(b"n")
    elif value.kind == "bool":
        hasher.update(b"t" if value.data else b"f")
    elif value.kind == "int":
        hasher.update(int(value.data).to_bytes(8, "little", signed=True))
    elif value.kind == "float":
        import struct

        hasher.update(struct.pack("<d", float(value.data)))
    elif value.kind == "string" or value.kind == "secret":
        hasher.update(b"s")
        hasher.update(len(value.data.encode("utf-8")).to_bytes(8, "little"))
    elif value.kind == "duration":
        hasher.update(b"d")
        hasher.update(int(value.data).to_bytes(8, "little"))
    elif value.kind == "quantity":
        hasher.update(b"q")
        hasher.update(value.unit.suffix.encode("utf-8"))
    elif value.kind == "table":
        hasher.update(b"T")
        for key in value.data:
            hasher.update(key.encode("utf-8"))
            hasher.update(b"\x00")
    elif value.kind == "array":
        hasher.update(b"A")
        hasher.update(len(value.data).to_bytes(8, "little"))
    elif value.kind == "blob":
        hasher.update(b"b")
        hasher.update(value.data.media_type.encode("utf-8"))
        hasher.update(b"t" if value.data.tainted else b"u")
