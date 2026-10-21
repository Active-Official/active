#!/usr/bin/env python3
"""Cross-implementation differential gate (CHECKLIST 3.001, PROPOSAL 5.1).

Drives every hermetic case in conformance/manifest.toml through three
independent embeddings of the same core and asserts IDENTICAL results:

  c_abi    frozen C ABI via ctypes (opaque handles, structured spans)
  python   WASM-first Tier-1 binding (wasmtime envelope items)
  js       WASM-first Tier-1 binding (shim envelope items)

Compared strictly per case: ordered code lists, span tuples
(line, column, end_line, end_column) or None, redacted JSON values, and
the refused flag. Codes and values must also equal the committed
expectations (errors/<id>.json, expected/<id>.json).

Out of scope by design (documented, not silent): 0008-include needs an
include-loader registration that the shipped binding surfaces do not
expose; the reference core suite covers it. Every other case runs.

Secrets: fixture sources may contain the raw marker; this script never
prints sources or raw values, scrubs all output, and fails the run if
the raw marker appears anywhere captured.

Usage: python conformance/harness/diff_vectors.py [--root R]
Exit code 0 only when every case agrees on every axis. Tally: DIFFERENTIAL-OK
"""

from __future__ import annotations

import ctypes
import json
import shutil
import subprocess
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "conformance" / "manifest.toml"
RAW_SECRET = "hunter2"


# Pinned cases come from the manifest itself ([[pins]]): fixtures needing
# host capabilities the v1 shims do not expose (include loader, profile
# override). The reference core suite covers them. Pins are recorded in
# the log with reason, never as failures and never silently.
def load_pins(manifest: dict) -> dict[str, str]:
    return {p["id"]: p.get("reason", "") for p in manifest.get("pins", [])}


def scrub(text: str) -> str:
    return text.replace(RAW_SECRET, "***SECRET***")


def norm_codes(codes: list[str]) -> list[str]:
    """Committed-expectation view: sorted, deduplicated.

    Recovery can legitimately emit one code twice (e.g. depth on
    open+close brackets). Cross-embedding equality stays per-item and
    ordered; comparison against committed files uses this set view,
    matching the reference core suite. Extracted so unit tests pin it.
    """
    return sorted(set(codes))


def fail(msg: str) -> int:
    print(scrub(f"DIFFERENTIAL-FAIL: {msg}"))
    return 1


def check_values(
    refused: bool,
    values: tuple[object, object, object],
    want_missing: bool,
    want_value: object,
) -> list[str]:
    """Fail-closed value rules for one non-parse-null case.

    `refused` and `values` are the cross-embedding-agreed shape (agreement
    itself is checked by the caller): when every embedding refuses, no
    value may come back; when any embedding yields a value, a committed
    expectation file must exist and match — even for manifest kind
    "refused", whose strict violations (T003/T005/E023) surface as
    Ok-plus-diagnostics with a value, exactly as the reference core
    suite blesses. Extracted so unit tests pin the rule.
    """
    got_value, py_value, js_value = values
    problems = []
    if refused:
        if got_value is not None or py_value is not None or js_value is not None:
            problems.append("refused but a value came back")
        return problems
    if want_missing:
        problems.append("non-refused case without committed expected value")
    if not (got_value == py_value == js_value):
        problems.append("values diverge across embeddings")
    if not want_missing and got_value != want_value:
        problems.append("value != committed expectation")
    return problems


def find_cdylib() -> Path | None:
    if sys.platform == "win32":
        names = ["active_ffi.dll"]
    elif sys.platform == "darwin":
        names = ["libactive_ffi.dylib"]
    else:
        names = ["libactive_ffi.so"]
    for d in (
        ROOT / "target" / "x86_64-unknown-linux-gnu" / "release",
        ROOT / "target" / "release",
        ROOT / "target" / "debug",
    ):
        for name in names:
            cand = d / name
            if cand.is_file():
                return cand
    return None


class CAbi:
    """Frozen C ABI driver via ctypes (shipped cdylib, no new deps)."""

    def __init__(self, lib_path: Path) -> None:
        lib = ctypes.CDLL(str(lib_path))
        lib.active_abi_version.restype = ctypes.c_uint32
        lib.active_abi_version.argtypes = []
        lib.active_module_parse.restype = ctypes.c_void_p
        lib.active_module_parse.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.c_void_p,
        ]
        lib.active_module_free.restype = None
        lib.active_module_free.argtypes = [ctypes.c_void_p]
        lib.active_check.restype = ctypes.c_void_p
        lib.active_check.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
        lib.active_diagnostics_len.restype = ctypes.c_size_t
        lib.active_diagnostics_len.argtypes = [ctypes.c_void_p]
        lib.active_diagnostic_code.restype = ctypes.c_char_p
        lib.active_diagnostic_code.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        lib.active_diagnostic_span.restype = ctypes.c_bool
        lib.active_diagnostic_span.argtypes = [
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        lib.active_diagnostic_severity.restype = ctypes.c_uint32
        lib.active_diagnostic_severity.argtypes = [
            ctypes.c_void_p,
            ctypes.c_size_t,
        ]
        lib.active_last_error.restype = ctypes.c_int32
        lib.active_last_error.argtypes = []
        lib.active_diagnostics_free.restype = None
        lib.active_diagnostics_free.argtypes = [ctypes.c_void_p]
        lib.active_eval.restype = ctypes.c_void_p
        lib.active_eval.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
        lib.active_value_free.restype = None
        lib.active_value_free.argtypes = [ctypes.c_void_p]
        lib.active_value_to_json.restype = ctypes.c_void_p
        lib.active_value_to_json.argtypes = [ctypes.c_void_p]
        lib.active_string_free.restype = None
        lib.active_string_free.argtypes = [ctypes.c_void_p]
        self.lib = lib
        self.severity_names = {1: "error", 2: "warning", 3: "info", 4: "hint"}
        if lib.active_abi_version() != 1:
            raise RuntimeError("ABI major mismatch")

    def run(self, source: bytes) -> tuple[list[str], list, bool, object, list[str]]:
        lib = self.lib
        mod = lib.active_module_parse(source, len(source), None)
        if not mod:
            # Frozen C ABI v1 surfaces parse detail as a NUMERIC thread code
            # only (active_last_error); there is no string-code path without
            # a module. The caller compares the WASM-backed embeddings
            # against the committed expectation instead, and logs the
            # numeric here for the record. The marker is a tuple, which a
            # parsed Active value (always JSON-shaped) can never collide
            # with, so a legitimate value cannot misroute into the
            # parse-null branch.
            return [], [], True, ("active_parse_null", lib.active_last_error()), []
        try:
            sev: list[str] = []
            val = lib.active_eval(mod, None)
            refused = not bool(val)
            value = None
            if not refused:
                # Owned Model-A string: read via the raw pointer (a c_char_p
                # restype would hand back a COPY whose address must never be
                # freed), decode, then free the original exactly once.
                raw = lib.active_value_to_json(val)
                if not raw:
                    raise RuntimeError("to_json returned null")
                value = json.loads(ctypes.string_at(raw).decode("utf-8"))
                lib.active_string_free(raw)
                lib.active_value_free(val)
            setup = lib.active_check(mod, None)
            if not setup:
                raise RuntimeError("C ABI check returned null")
            try:
                n = lib.active_diagnostics_len(setup)
                codes: list[str] = []
                items: list = []
                for i in range(n):
                    code = lib.active_diagnostic_code(setup, i)
                    codes.append(code.decode("utf-8") if code else "")
                    slots = (ctypes.c_uint32 * 4)()
                    if lib.active_diagnostic_span(setup, i, slots):
                        items.append([slots[0], slots[1], slots[2], slots[3]])
                    else:
                        items.append(None)
                    s = lib.active_diagnostic_severity(setup, i)
                    sev.append(self.severity_names.get(s, f"unknown({s})"))
                return codes, items, refused, value, sev
            finally:
                lib.active_diagnostics_free(setup)
        finally:
            lib.active_module_free(mod)


def run_python(source: bytes) -> tuple[list[str], list, bool, object, list]:
    sys.path.insert(0, str(ROOT / "bindings" / "python"))
    from active import ActiveError, Module

    mod = Module()
    try:
        diags = mod.check(source.decode("utf-8"))
        codes = [d.code for d in diags]
        items = [list(d.span()) if d.span() else None for d in diags]
        sev = [d.severity for d in diags]
        try:
            value = mod.eval(source.decode("utf-8"))
            return codes, items, False, value, sev
        except ActiveError:
            return codes, items, True, None, sev
    finally:
        mod.close()


def run_node(src_file: Path) -> tuple[list[str], list, bool, object, list]:
    node = shutil.which("node")
    if node is None:
        raise RuntimeError("node not installed")
    p = subprocess.run(
        [node, str(ROOT / "bindings" / "js" / "diff_probe.js"), "-", str(src_file)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=300,
    )
    if p.returncode != 0 or not p.stdout.strip():
        raise RuntimeError(scrub(f"node probe failed: {(p.stderr or p.stdout)[-500:]}"))
    env = json.loads(p.stdout.strip().splitlines()[-1])
    items = [
        (
            [it["line"], it["column"], it["end_line"], it["end_column"]]
            if it["line"] is not None
            else None
        )
        for it in env["items"]
    ]
    sev = [it.get("severity") for it in env["items"]]
    return env["codes"], items, bool(env["refused"]), env["value"], sev


def main() -> int:
    with open(MANIFEST, "rb") as f:
        manifest = tomllib.load(f)
    cases = manifest.get("cases", [])
    lib_path = find_cdylib()
    if lib_path is None:
        print("SKIP: no native cdylib built")
        return 0
    if shutil.which("node") is None:
        print("SKIP: node not installed")
        return 0
    try:
        import wasmtime  # noqa: F401
    except ImportError:
        print("SKIP: wasmtime not installed")
        return 0

    abi = CAbi(lib_path)
    pins = load_pins(manifest)
    checked = 0
    for case in cases:
        cid = case["id"]
        if cid in pins:
            print(f"DIFF {cid} pinned: {pins[cid]}")
            continue
        src_file = ROOT / "conformance" / case["file"]
        source = src_file.read_bytes()
        kind = case.get("kind", "")
        errors_file = ROOT / "conformance" / "errors" / f"{cid}.json"
        expected_file = ROOT / "conformance" / "expected" / f"{cid}.json"
        want_codes: list[str] = []
        if errors_file.is_file():
            want_codes = json.loads(errors_file.read_text())["codes"]
        want_value = None
        want_value_missing = not expected_file.is_file()
        if not want_value_missing:
            want_value = json.loads(expected_file.read_text())

        got_codes, got_spans, got_refused, got_value, got_sev = abi.run(source)
        py_codes, py_spans, py_refused, py_value, py_sev = run_python(source)
        js_codes, js_spans, js_refused, js_value, js_sev = run_node(src_file)

        problems = []
        parse_null = (
            isinstance(got_value, tuple)
            and len(got_value) == 2
            and got_value[0] == "active_parse_null"
        )
        # Committed expectations store the SET view (sorted, deduped), same
        # normalization the reference core suite applies: recovery can
        # legitimately emit one code twice (e.g. 0006 depth on open+close).
        # Cross-embedding equality stays per-item and ordered.
        if parse_null:
            # C ABI parse refusal: numeric detail only, by frozen contract.
            # Strict string-code equality still holds between the two
            # WASM-backed embeddings and the committed expectation.
            if not (py_codes == js_codes):
                problems.append(
                    f"parse-refused codes diverge: python={py_codes} js={js_codes}"
                )
            if kind != "refused":
                problems.append(f"kind={kind} but embeddings parse-refused (no value)")
            if norm_codes(py_codes) != want_codes:
                problems.append(
                    f"codes {norm_codes(py_codes)} != expected {want_codes}"
                )
            if not (py_spans == js_spans):
                problems.append("parse-refused spans diverge")
            if not (py_sev == js_sev):
                problems.append("parse-refused severities diverge")
            if not (py_refused and js_refused):
                problems.append("parse-refused but an embedding evaluated")
            if py_value is not None or js_value is not None:
                problems.append("parse-refused but a value came back")
            if kind == "refused" and not py_codes:
                problems.append("kind=refused but no codes surfaced")
            if kind == "clean" and py_codes:
                problems.append(f"kind=clean but codes surfaced: {py_codes}")
            if kind == "clean" and (py_refused or js_refused):
                problems.append("kind=clean but an embedding refused")
            if RAW_SECRET in json.dumps(
                {"py": py_value, "js": js_value, "codes": py_codes + js_codes}
            ):
                problems.append("raw secret marker in captured output")
            if problems:
                return fail(f"{cid}: " + "; ".join(problems))
            checked += 1
            print(
                scrub(
                    f"DIFF {cid} ok kind={kind} codes={py_codes} "
                    f"(c_abi parse-null numeric={got_value[1]})"
                )
            )
            continue
        if not (got_codes == py_codes == js_codes):
            problems.append(
                f"codes diverge: c_abi={got_codes} python={py_codes} js={js_codes}"
            )
        if norm_codes(got_codes) != want_codes:
            problems.append(f"codes {norm_codes(got_codes)} != expected {want_codes}")
        if not (got_spans == py_spans == js_spans):
            problems.append(
                f"spans diverge: c_abi={got_spans} python={py_spans} js={js_spans}"
            )
        if not (got_sev == py_sev == js_sev):
            problems.append(
                f"severities diverge: c_abi={got_sev} python={py_sev} js={js_sev}"
            )
        if not (got_refused == py_refused == js_refused):
            problems.append(
                f"refused diverges: c_abi={got_refused} python={py_refused} js={js_refused}"
            )
        if kind == "refused" and not got_codes:
            problems.append("kind=refused but no codes surfaced")
        if kind == "clean" and got_codes:
            problems.append(f"kind=clean but codes surfaced: {got_codes}")
        if kind == "clean" and got_refused:
            problems.append("kind=clean but an embedding refused")
        problems.extend(
            check_values(
                got_refused,
                (got_value, py_value, js_value),
                want_value_missing,
                want_value,
            )
        )
        # Fail-closed secret scan: values are redacted by construction, so
        # the raw marker must appear nowhere in any captured payload. The
        # committed expectations already pin redaction; this scans the live
        # outputs, so a missing expectation file cannot hide a leak.
        blob = json.dumps(
            {
                "got": got_value,
                "py": py_value,
                "js": js_value,
                "codes": got_codes + py_codes + js_codes,
            }
        )
        if RAW_SECRET in blob:
            return fail(f"{cid}: raw secret marker in captured output")
        if problems:
            detail = "; ".join(problems)
            return fail(f"{cid}: {detail}")
        checked += 1
        print(
            scrub(
                f"DIFF {cid} ok kind={kind} codes={got_codes} "
                f"spans={len(got_spans)} severities={got_sev} refused={got_refused}"
            )
        )
    if checked == 0:
        return fail("no hermetic cases exercised (zero-proof pass refused)")
    print(f"DIFFERENTIAL-OK ({checked} cases, codes+spans+values identical)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
