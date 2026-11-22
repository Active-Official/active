"""Unit table and literal canonicalization (spec Chapter 4, G18/G06).

A unit literal canonicalizes exactly as the shipped engine states:
time -> DurationMs, bytes -> Int, percent -> a dimensionless ratio,
everything else -> Quantity in its canonical unit. Literal magnitudes
clamp at zero; explicit unary negation is handled by the evaluator.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


@dataclass(frozen=True)
class Unit:
    suffix: str  # canonical suffix
    dimension: str  # equality/arithmetic dimension key
    factor: float = 1.0
    offset: float = 0.0  # affine only
    kind: str = "quantity"  # duration | bytes | quantity
    special: str | None = None  # percent | affine | log | retained


def _u(
    suffix: str,
    dimension: str,
    factor: float = 1.0,
    kind: str = "quantity",
    special: str | None = None,
) -> Unit:
    return Unit(suffix, dimension, factor, 0.0, kind, special)


UNITS: dict[str, Unit] = {}


def _add(unit: Unit, *aliases: str) -> None:
    UNITS[unit.suffix] = unit
    for alias in aliases:
        UNITS[alias] = unit


# Time -> DurationMs.
_add(
    _u("ns", "ms", 1e-6, "duration"),
)
_add(_u("us", "ms", 1e-3, "duration"), "μs")
_add(_u("ms", "ms", 1.0, "duration"))
_add(_u("s", "ms", 1000.0, "duration"), "sec")
_add(_u("m", "ms", 60_000.0, "duration"), "min")
_add(_u("h", "ms", 3_600_000.0, "duration"))
_add(_u("d", "ms", 86_400_000.0, "duration"))
_add(_u("week", "ms", 604_800_000.0, "duration"))

# Bytes -> Int.
_add(_u("B", "B", 1.0, "bytes"))
_add(_u("KB", "B", 1e3, "bytes"))
_add(_u("MB", "B", 1e6, "bytes"))
_add(_u("GB", "B", 1e9, "bytes"))
_add(_u("TB", "B", 1e12, "bytes"))
_add(_u("KiB", "B", 2**10, "bytes"))
_add(_u("MiB", "B", 2**20, "bytes"))
_add(_u("GiB", "B", 2**30, "bytes"))
_add(_u("TiB", "B", 2**40, "bytes"))
_add(_u("PiB", "B", 2**50, "bytes"))
_add(_u("EiB", "B", 2**60, "bytes"))

# Bits and data rates -> Quantity.
_add(_u("b", "b", 1.0))
_add(_u("Kb", "b", 1024.0))
_add(_u("Mb", "b", 2**20))
_add(_u("Gb", "b", 2**30))
_add(_u("bps", "bps", 1.0))
_add(_u("Kbps", "bps", 1e3))
_add(_u("Mbps", "bps", 1e6))
_add(_u("Gbps", "bps", 1e9))
_add(_u("MB/s", "bps", 8e6))

# Percent -> dimensionless ratio (stored value/100).
_add(_u("%", "%", 0.01, special="percent"))

# Frequency.
_add(_u("Hz", "Hz", 1.0))
_add(_u("kHz", "Hz", 1e3))
_add(_u("MHz", "Hz", 1e6))
_add(_u("GHz", "Hz", 1e9))
_add(_u("THz", "Hz", 1e12))

# Rates are retained, never cross-normalized (spec §4.1).
_add(_u("/s", "/s", 1.0, special="retained"), "per_s")
_add(_u("/m", "/m", 1.0, special="retained"), "per_m")
_add(_u("/h", "/h", 1.0, special="retained"), "per_h")
_add(_u("req/s", "req/s", 1.0, special="retained"), "req_s", "/req/s")
_add(_u("req/m", "req/m", 1.0, special="retained"), "req_m", "/req/m")
_add(_u("req/h", "req/h", 1.0, special="retained"), "req_h", "/req/h")
_add(_u("tok/s", "tok/s", 1.0, special="retained"), "tok_s")

# Tokens and compute.
_add(_u("tokens", "tokens", 1.0))
_add(_u("ktokens", "tokens", 1e3))
_add(_u("Mtokens", "tokens", 1e6))
_add(_u("FLOPS", "FLOPS", 1.0))
_add(_u("TFLOPS", "FLOPS", 1e12))
_add(_u("PFLOPS", "FLOPS", 1e15))

# Pressure, electrical, energy, force.
_add(_u("Pa", "Pa", 1.0))
_add(_u("bar", "Pa", 100_000.0))
_add(_u("psi", "Pa", 6894.757293168))
_add(_u("V", "V", 1.0))
_add(_u("A", "A", 1.0))
_add(_u("W", "W", 1.0))
_add(_u("J", "J", 1.0))
_add(_u("kWh", "J", 3_600_000.0))
_add(_u("N", "N", 1.0))

# Angle, rotation, length, mass.
_add(_u("rad", "rad", 1.0))
_add(_u("deg", "rad", math.pi / 180.0))
_add(_u("rpm", "rpm", 1.0, special="retained"))
_add(_u("meter", "meter", 1.0))
_add(_u("km", "meter", 1000.0))
_add(_u("kg", "kg", 1.0))
_add(_u("g", "kg", 0.001))

# Temperature (affine) and decibels (log).
_add(_u("K", "K", 1.0, special="affine"))
_add(_u("Cel", "K", 1.0, special="affine"))
_add(_u("Fahr", "K", 1.0, special="affine"))
_add(_u("dB", "dB", 1.0, special="log"))

# Aliases whose canonical spelling differs from their source spelling.
UNITS["per_s"] = UNITS["/s"]
UNITS["per_m"] = UNITS["/m"]
UNITS["per_h"] = UNITS["/h"]
UNITS["req_s"] = UNITS["req/s"]
UNITS["req_m"] = UNITS["req/m"]
UNITS["req_h"] = UNITS["req/h"]
UNITS["tok_s"] = UNITS["tok/s"]

# Unit characters: identifier chars plus '/' and '%'.
UNIT_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_/%μµ")


@dataclass(frozen=True)
class Canonical:
    kind: str  # duration | int | quantity
    value: float
    unit: Unit | None = None

    def as_int(self) -> int:
        return int(self.value)


def _clamp_i64(value: int) -> int:
    return max(I64_MIN, min(I64_MAX, value))


def _clamp_u64(value: int) -> int:
    return max(0, min(U64_MAX, value))


_CANONICAL_UNIT = {
    "kHz": "Hz",
    "MHz": "Hz",
    "GHz": "Hz",
    "THz": "Hz",
    "Kb": "b",
    "Mb": "b",
    "Gb": "b",
    "Kbps": "bps",
    "Mbps": "bps",
    "Gbps": "bps",
    "MB/s": "bps",
    "bar": "Pa",
    "psi": "Pa",
    "kWh": "J",
    "deg": "rad",
    "km": "meter",
    "g": "kg",
    "Cel": "K",
    "Fahr": "K",
}


def canonical_unit(unit: Unit) -> Unit:
    return UNITS[_CANONICAL_UNIT.get(unit.suffix, unit.suffix)]


def canonicalize(amount: float, unit: Unit) -> Canonical:
    """Canonical form of a non-negative literal amount in `unit`."""
    if unit.kind == "duration":
        ms = round(amount * unit.factor)
        return Canonical("duration", float(_clamp_u64(ms)))
    if unit.kind == "bytes":
        n = round(amount * unit.factor)
        return Canonical("int", float(_clamp_i64(n)))
    if unit.special == "percent":
        return Canonical("quantity", amount * unit.factor, canonical_unit(unit))
    if unit.suffix == "Cel":
        return Canonical("quantity", amount + 273.15, UNITS["K"])
    if unit.suffix == "Fahr":
        return Canonical("quantity", (amount - 32.0) * 5.0 / 9.0 + 273.15, UNITS["K"])
    return Canonical("quantity", amount * unit.factor, canonical_unit(unit))


def canonical_amount(value: float, unit: Unit) -> float:
    """Canonical numeric magnitude for a value already in `unit`."""
    if unit.kind == "duration":
        return float(round(value * unit.factor))
    if unit.kind == "bytes":
        return float(round(value * unit.factor))
    if unit.suffix == "Cel":
        return value + 273.15
    if unit.suffix == "Fahr":
        return (value - 32.0) * 5.0 / 9.0 + 273.15
    return value * unit.factor


def dimension(unit: Unit) -> str:
    return unit.dimension


def same_dimension(a: Unit, b: Unit) -> bool:
    return a.dimension == b.dimension


def affine_or_log(unit: Unit) -> bool:
    return unit.special in ("affine", "log")


def is_percent(unit: Unit) -> bool:
    return unit.suffix == "%"


def format_amount(value: float, unit: Unit) -> str:
    """Source/TOML spelling of a canonical magnitude (percent -> 25%)."""
    if unit.suffix == "%":
        return _number(value * 100.0) + "%"
    if unit.kind == "duration":
        return str(round(value)) + unit.suffix
    if unit.kind == "bytes":
        return str(round(value)) + unit.suffix
    return _number(value) + unit.suffix


def _number(value: float) -> str:
    if math.isfinite(value) and value == int(value):
        return str(int(value))
    return repr(value)
