"""Canonical JSON export, value hashes, and the shipped float spelling."""

from __future__ import annotations

import hashlib
import json
import math
from typing import Any

from .model import Value

MARKER = "***SECRET***"


def float_text(value: float) -> str:
    """The shipped writer's spelling (G13): shortest round-trip form,
    fixed notation while the decimal exponent is in `-5..=15`, otherwise
    scientific with an explicit `+` on positive exponents and no zero
    padding; `-0.0` renders `0.0`.

    Python's `repr` supplies the shortest digits, but its notation
    thresholds differ from the Rust writer's, so this normalizes to a
    (digits, exponent) pair and re-renders both styles.
    """
    if value == 0.0:
        return "0.0"
    if math.isnan(value):
        return "nan"
    if math.isinf(value):
        return "inf" if value > 0 else "-inf"
    sign = "-" if value < 0.0 else ""
    text = repr(abs(float(value)))
    if "e" in text:
        mantissa, exp = text.split("e")
        integer, _, fraction = mantissa.partition(".")
        digits = (integer + fraction).rstrip("0") or "0"
        exponent = int(exp) + len(integer.lstrip("0")) - 1
    else:
        integer, _, fraction = text.partition(".")
        if integer.strip("0"):
            digits = (integer + fraction).rstrip("0")
            exponent = len(integer) - 1
        else:
            leading = len(fraction) - len(fraction.lstrip("0"))
            digits = fraction.lstrip("0").rstrip("0") or "0"
            exponent = -(leading + 1)
    if -5 <= exponent < 16:
        if exponent >= 0:
            if len(digits) <= exponent + 1:
                body = digits + "0" * (exponent + 1 - len(digits)) + ".0"
            else:
                body = digits[: exponent + 1] + "." + digits[exponent + 1 :]
        else:
            body = "0." + "0" * (-exponent - 1) + digits
        return sign + body
    mantissa = digits[0] + ("." + digits[1:] if len(digits) > 1 else "")
    return f"{sign}{mantissa}e{exponent:+d}"


def _seeds(seeds: tuple[str, ...] | list[str]) -> list[str]:
    return sorted(
        {s for s in seeds if s}, key=lambda s: len(s.encode("utf-8")), reverse=True
    )


def _redact_text(text: str, seeds: list[str]) -> str:
    """Exact matches at any length; substring seeds only at >= 4 bytes."""
    if any(seed == text for seed in seeds):
        return MARKER
    for seed in seeds:
        if len(seed.encode("utf-8")) >= 4 and seed in text:
            text = text.replace(seed, MARKER)
    return text


def _redact_key(key: str, seeds: list[str]) -> str:
    """A material key slot is replaced whole (spec/secrets.md §2.1)."""
    if any(
        seed == key or (len(seed.encode("utf-8")) >= 4 and seed in key)
        for seed in seeds
    ):
        return MARKER
    return key


def to_json_value(
    value: Value, *, redact: bool = True, seeds: tuple[str, ...] | list[str] = ()
) -> Any:
    kind = value.kind
    seed_list = _seeds(seeds)
    # P37-N2: a derived scalar is material even when its kind is benign.
    # Containers carry the flag on their leaves, so only leaves render it.
    if value.material and kind not in ("secret", "blob", "array", "table"):
        return MARKER
    if kind == "null":
        return None
    if kind == "bool":
        return bool(value.data)
    if kind == "int":
        return int(value.data)
    if kind == "float":
        if math.isnan(value.data):
            return {"$active.float": "nan"}
        if math.isinf(value.data):
            return {"$active.float": "inf" if value.data > 0 else "-inf"}
        return float(value.data)
    if kind == "string":
        return _redact_text(value.data, seed_list)
    if kind == "duration":
        return {"$active.duration": {"ms": int(value.data)}}
    if kind == "quantity":
        return {
            "$active.quantity": {"unit": value.unit.suffix, "value": float(value.data)}
        }
    if kind == "array":
        return [to_json_value(item, redact=redact, seeds=seeds) for item in value.data]
    if kind == "table":
        return {
            _redact_key(key, seed_list): to_json_value(item, redact=redact, seeds=seeds)
            for key, item in value.data.items()
        }
    if kind == "secret":
        return MARKER
    if kind == "blob":
        blob = value.data
        media = MARKER if blob.tainted and redact else blob.media_type
        return {
            "$active.blob": {
                "hash": blob.hash,
                "size": blob.size,
                "media_type": media,
                "tainted": blob.tainted,
            }
        }
    return None


def _render(value: Any, indent: int, level: int) -> str:
    pad = " " * (indent * level)
    child = " " * (indent * (level + 1))
    if isinstance(value, dict):
        if not value:
            return "{}"
        items = []
        for key, item in value.items():
            items.append(
                f"{child}{json.dumps(key)}: {_render(item, indent, level + 1)}"
            )
        return "{\n" + ",\n".join(items) + "\n" + pad + "}"
    if isinstance(value, list):
        if not value:
            return "[]"
        items = [child + _render(item, indent, level + 1) for item in value]
        return "[\n" + ",\n".join(items) + "\n" + pad + "]"
    if isinstance(value, float):
        return float_text(value)
    return json.dumps(value)


def pretty_json(
    value: Value, indent: int = 2, seeds: tuple[str, ...] | list[str] = ()
) -> str:
    return _render(to_json_value(value, seeds=seeds), indent, 0)


def canonical_json(value: Value, seeds: tuple[str, ...] | list[str] = ()) -> str:
    """Compact, recursively key-sorted canonical JSON (G13/G14)."""
    return _canonical(to_json_value(value, seeds=seeds))


def _canonical(value: Any) -> str:
    if isinstance(value, dict):
        parts = [
            json.dumps(key) + ":" + _canonical(value[key])
            for key in sorted(value.keys(), key=lambda k: k.encode("utf-8"))
        ]
        return "{" + ",".join(parts) + "}"
    if isinstance(value, list):
        return "[" + ",".join(_canonical(item) for item in value) + "]"
    if isinstance(value, float):
        return float_text(value)
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    return json.dumps(value, ensure_ascii=False)


def content_hash(value: Value, seeds: tuple[str, ...] | list[str] = ()) -> str:
    canonical = canonical_json(value, seeds=seeds)
    return "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()
