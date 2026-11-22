"""Include resolution and merge semantics (spec grammar §2.7, G11)."""

from __future__ import annotations

import hashlib
import os
import stat

from .diag import Diagnostic, error
from .eval import Engine
from .model import Module, Node, SchemaField, SchemaTable, SchemaType
from .parser import parse

MAX_DEPTH = 32
MAX_INCLUDES = 256
MAX_INCLUDED_BYTES = 16 * 1024 * 1024


def resolve_includes(
    module: Module,
    base_file: str,
    engine: Engine,
    stack: list[str] | None = None,
    state: dict | None = None,
) -> None:
    stack = stack or []
    state = state if state is not None else {"count": 0, "bytes": 0}
    full_base = os.path.abspath(base_file)
    stack = stack + [os.path.normcase(os.path.realpath(full_base))]
    include_roots: list[tuple[int, str]] = []
    for stmt in module.includes:
        path = stmt.path
        normalized = path.replace("\\", "/")
        checked = normalized
        if len(path) >= 2 and path[0].isalpha() and path[1] == ":":
            checked = normalized[2:]
        if (
            os.path.isabs(path)
            or checked.startswith("/")
            or (
                len(path) >= 3
                and path[0].isalpha()
                and path[1] == ":"
                and path[2] in ("/", "\\")
            )
            or ".." in checked.split("/")
        ):
            module.diagnostics.append(
                error("E015", "include path escapes the directory", span=stmt.span)
            )
            continue
        full = os.path.normpath(os.path.join(os.path.dirname(full_base), path))
        base_real = os.path.normcase(os.path.realpath(os.path.dirname(full_base)))
        full_real = os.path.normcase(os.path.realpath(full))
        try:
            contained = os.path.commonpath([base_real, full_real]) == base_real
        except ValueError:
            contained = False
        # A drive-relative name (`a:b`) is a plain filename on the shipped
        # loader: it fails at open with E017, not as an escape.
        if ":" in path and not (
            len(path) >= 3
            and path[0].isalpha()
            and path[1] == ":"
            and path[2] in ("/", "\\")
        ):
            contained = True
        if not contained:
            module.diagnostics.append(
                error("E015", "include path escapes the directory", span=stmt.span)
            )
            continue
        key = os.path.normcase(full_real)
        if key in stack:
            module.diagnostics.append(error("E016", "include cycle", span=stmt.span))
            continue
        # stack holds the base plus every include on the path; the
        # shipped limit refuses the 33rd include.
        if len(stack) > MAX_DEPTH:
            module.diagnostics.append(
                error("E016", "include depth exceeded", span=stmt.span)
            )
            continue
        state["count"] += 1
        if state["count"] > MAX_INCLUDES:
            module.diagnostics.append(
                error("E028", "include fan-out budget exceeded", span=stmt.span)
            )
            continue
        remaining = MAX_INCLUDED_BYTES - state["bytes"]
        flags = os.O_RDONLY | getattr(os, "O_BINARY", 0)
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        try:
            fd = os.open(full, flags)
            try:
                if not stat.S_ISREG(os.fstat(fd).st_mode):
                    module.diagnostics.append(
                        error("E017", f"cannot read include `{path}`", span=stmt.span)
                    )
                    continue
                # Linux exposes the opened inode; verify containment on
                # the descriptor, not the raced path.
                proc = f"/proc/self/fd/{fd}"
                if os.path.exists(proc):
                    opened = os.path.normcase(os.path.realpath(proc))
                    if os.path.commonpath([base_real, opened]) != base_real:
                        module.diagnostics.append(
                            error(
                                "E015",
                                "include path escapes the directory",
                                span=stmt.span,
                            )
                        )
                        continue
                raw = os.read(fd, remaining + 1)
            finally:
                os.close(fd)
        except OSError:
            module.diagnostics.append(
                error("E017", f"cannot read include `{path}`", span=stmt.span)
            )
            continue
        if len(raw) > remaining:
            module.diagnostics.append(
                error("E028", "include byte budget exceeded", span=stmt.span)
            )
            continue
        state["bytes"] += len(raw)
        if state["bytes"] > MAX_INCLUDED_BYTES:
            module.diagnostics.append(
                error("E028", "include byte budget exceeded", span=stmt.span)
            )
            continue
        digest = hashlib.sha256(raw).hexdigest()
        if stmt.pin and digest.lower() != stmt.pin.lower():
            module.diagnostics.append(
                error("E041", f"include pin mismatch for `{path}`", span=stmt.span)
            )
            continue
        try:
            source = raw.decode("utf-8")
        except UnicodeDecodeError:
            module.diagnostics.append(
                error("E017", f"cannot parse include `{path}`", span=stmt.span)
            )
            continue
        try:
            included = parse(source, full)
        except Exception as failure:  # noqa: BLE001 - parse failures become diagnostics
            diags = getattr(failure, "diagnostics", None)
            if diags:
                module.diagnostics.extend(diags)
            else:
                module.diagnostics.append(
                    error("E017", f"cannot parse include `{path}`", span=stmt.span)
                )
            continue
        resolve_includes(included, full, engine, stack, state)
        root, mapping = _clone_module(
            module, included, stmt.span.line if stmt.span else 0
        )
        included_fields = dict(module.get(root).fields)
        if not stmt.alias:
            # Mode and capability markers are host decisions: an included
            # library never flips the composed root's metadata.
            included_root = module.get(root)
            for meta in ("active_mode", "active_capabilities"):
                included_root.fields.pop(meta, None)
                module.bindings.pop(meta, None)
                included_fields.pop(meta, None)
        if stmt.alias:
            alias = module.add(Node("table", fields={stmt.alias: root}))
            module.bindings.setdefault(stmt.alias, alias)
            if included.schema is not None:
                for name, field in included.schema.fields.items():
                    if name in included_fields or not _field_required(field):
                        continue
                    module.diagnostics.append(
                        error(
                            "T004",
                            f"missing required key '{name}' (schema in include)",
                            path=f"{stmt.alias}.{name}",
                        )
                    )
        else:
            for name, child in included_fields.items():
                module.bindings.setdefault(name, child)
        _merge_included_module(
            module,
            included,
            mapping,
            set(included_fields),
            include_schema=not stmt.alias,
            include_line=stmt.span.line if stmt.span else 0,
        )
        prefix = f"{stmt.alias}." if stmt.alias else ""
        for immutable in included.immutable_paths:
            module.immutable_paths.add(prefix + immutable)
        if not stmt.private:
            if stmt.alias:
                include_roots.append((alias, path))
            else:
                include_roots.append((root, path))
        module.diagnostics.extend(included.diagnostics)

    _resolve_after_includes(module)
    module.patches.sort(
        key=lambda patch: (
            patch.order
            if patch.order is not None
            else ((patch.span.line if patch.span else 0), 0)
        )
    )
    if include_roots:
        accum = module.add(Node("table"))
        module.internal_ids.add(accum)
        for root, path in include_roots:
            _merge_into(module, accum, root, path, module.diagnostics, conflict=True)
        _merge_local_wins(module, module.root, accum)


def _clone_module(
    main: Module, inc: Module, include_line: int
) -> tuple[int, dict[int, int]]:
    mapping: dict[int, int] = {}
    for old_id, node in enumerate(inc.nodes):
        mapping[old_id] = main.add(
            Node(
                node.kind,
                value=node.value,
                name=node.name,
                op=node.op,
                unit=node.unit,
                span=node.span,
                include_line=include_line,
            )
        )
    for old_id, node in enumerate(inc.nodes):
        new = main.get(mapping[old_id])
        new.children = [mapping[c] for c in node.children if c in mapping]
        new.fields = {k: mapping[v] for k, v in node.fields.items() if v in mapping}
        parts = []
        for kind, part in node.parts:
            if kind == "expr" and isinstance(part, int) and part in mapping:
                parts.append((kind, mapping[part]))
            else:
                parts.append((kind, part))
        new.parts = parts
    targets = getattr(inc, "ident_target", {})
    if not hasattr(main, "ident_target"):
        main.ident_target = {}
    for old, target in targets.items():
        if old in mapping and target in mapping:
            main.ident_target[mapping[old]] = mapping[target]
    for name, node_id in inc.bindings.items():
        main.bindings.setdefault(name, mapping[node_id])
    for name, node_id in inc.profiles.items():
        main.profiles.setdefault(name, mapping[node_id])
    return mapping[inc.root], mapping


def _remap_type(typ: SchemaType, mapping: dict[int, int]) -> SchemaType:
    return SchemaType(
        typ.kind,
        element=_remap_type(typ.element, mapping) if typ.element else None,
        fields={
            name: _remap_field(field, mapping) for name, field in typ.fields.items()
        },
        closed=typ.closed,
        members=list(typ.members),
        branches=[_remap_type(branch, mapping) for branch in typ.branches],
    )


def _remap_field(field: SchemaField, mapping: dict[int, int]) -> SchemaField:
    return SchemaField(
        _remap_type(field.typ, mapping),
        args={name: mapping.get(node, node) for name, node in field.args.items()},
        description=field.description,
        deprecated=field.deprecated,
        removed_in=field.removed_in,
        requires=field.requires,
        blob_media_type=field.blob_media_type,
        blob_max_size=field.blob_max_size,
    )


def _field_required(field: SchemaField) -> bool:
    return field.typ.kind != "Null" and "default" not in field.args


def _resolve_after_includes(module: Module) -> None:
    """Bind identifiers that an include made referenceable.

    The parser resolves identifiers against the bindings known at parse
    time; included values and private includes arrive later. Resolve
    those now and withdraw the W001 the parser recorded for them.
    """
    if not hasattr(module, "ident_target"):
        module.ident_target = {}
    changed: set[str] = set()
    for node_id, node in enumerate(module.nodes):
        if node.kind != "ident" or node_id in module.ident_target:
            continue
        target = module.bindings.get(node.name)
        if target is not None:
            module.ident_target[node_id] = target
            changed.add(node.name)
    if not changed:
        return
    module.diagnostics = [
        diagnostic
        for diagnostic in module.diagnostics
        if not (
            diagnostic.code == "W001"
            and any(
                f"`{name}`" in diagnostic.message or f"'{name}'" in diagnostic.message
                for name in changed
            )
        )
    ]


def _merge_included_module(
    main: Module,
    included: Module,
    mapping: dict[int, int],
    included_keys: set[str],
    *,
    include_schema: bool,
    include_line: int,
) -> None:
    """Carry an included file's schema, patches, and evolution metadata.

    The included root itself merges as data; these are module-level
    declarations, and the shipped engine enforces them on the composed
    document. Local declarations win on a same-named field.
    """
    if include_schema and included.schema is not None:
        # An included schema constrains keys the composed document
        # supplies. The shipped engine does not make an included schema's
        # absent field required (probed): a missing key is not T004.
        present = set(main.get(main.root).fields) | included_keys
        remapped = {
            name: _remap_field(field, mapping)
            for name, field in included.schema.fields.items()
            if name in present
        }
        checks = [
            (name, mapping.get(expr, expr)) for name, expr in included.schema.checks
        ]
        if main.schema is None:
            main.schema = SchemaTable(
                fields=remapped, closed=included.schema.closed, checks=checks
            )
        else:
            for name, field in remapped.items():
                main.schema.fields.setdefault(name, field)
            main.schema.closed = main.schema.closed or included.schema.closed
            main.schema.checks.extend(checks)
    for index, patch in enumerate(included.patches):
        main.patches.append(
            type(patch)(
                patch.kind,
                patch.path,
                value=mapping.get(patch.value, patch.value),
                new_name=patch.new_name,
                span=patch.span,
                order=patch.order if patch.order is not None else (include_line, index),
            )
        )
    for annotation in included.annotations:
        main.annotations.append(
            type(annotation)(
                annotation.kind,
                annotation.payload,
                target=mapping.get(annotation.target, annotation.target),
            )
        )
    for slot in ("schema_version", "schema_min_reader", "schema_min_writer"):
        if getattr(main, slot, None) is None:
            value = getattr(included, slot, None)
            if value is not None:
                setattr(main, slot, value)


def _join_path(prefix: str, key: str) -> str:
    return f"{prefix}.{key}" if prefix else key


def _literal_equal(main: Module, a: int, b: int) -> bool:
    na = main.get(a)
    nb = main.get(b)
    return na.kind == "literal" and nb.kind == "literal" and na.value == nb.value


def _merge_into(
    main: Module,
    target: int,
    source: int,
    label: str,
    diags: list[Diagnostic],
    conflict: bool,
    prefix: str = "",
) -> None:
    t = main.get(target)
    s = main.get(source)
    for key, child in s.fields.items():
        if key not in t.fields:
            t.fields[key] = child
            continue
        a = main.get(t.fields[key])
        b = main.get(child)
        if a.kind == "table" and b.kind == "table":
            _merge_into(
                main,
                t.fields[key],
                child,
                label,
                diags,
                conflict,
                _join_path(prefix, key),
            )
        elif conflict and not _literal_equal(main, t.fields[key], child):
            diags.append(
                error(
                    "E046",
                    f"merge conflict at `{key}` (include {label})",
                    path=_join_path(prefix, key),
                    span=b.span,
                )
            )


def _merge_local_wins(main: Module, local: int, included: int) -> None:
    lnode = main.get(local)
    inode = main.get(included)
    for key, child in inode.fields.items():
        if key not in lnode.fields:
            lnode.fields[key] = child
            continue
        a = main.get(lnode.fields[key])
        b = main.get(child)
        if a.kind == "table" and b.kind == "table":
            _merge_local_wins(main, lnode.fields[key], child)
