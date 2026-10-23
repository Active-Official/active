# Mode Doctrine Implementation (normative)

**Version:** 1.0
**Back-reference:** PROPOSAL §0.2, §3.4; CHECKLIST Phase M (M.001–M.007).
**Status:** Normative. This document fixes the mode marker, Config compatibility, Data Mode behavior, and the mode diagnostics.

---

## 1. Marker syntax (M.002)

One reserved root-level key selects the mode for the whole document:

```active
active_mode = "data"
port = 8080
```

- `"config"` (or an absent marker) selects Config Mode.
- `"data"` selects Data Mode.
- Anything else (including non-string values) is **E024**.
- The marker stays a regular value key: Data documents are
  self-describing and the marker flows into evaluated JSON (which
  the agent-interchange profile mandates).
- Included files cannot set the mode; only the top-level document's
  marker counts (local document wins, includes inherit nothing).
- `active_mode` never counts as an unexpected key in a root-level
  closed table — it is metadata, not data. Nested tables keep the
  strict rule: a nested `active_mode` key is an ordinary key there.

## 2. Config compatibility (M.003)

Documents without a marker parse and evaluate EXACTLY as before:
`Module::mode` defaults to Config, no new diagnostics fire, and the
value tree is byte-identical to the pre-mode implementation (pinned
by the full pre-existing suite plus an explicit equality test).

## 3. Data Mode behavior (M.004)

- **Progressive warnings:** at 80% of any nonzero budget limit, Data
  Mode evaluation emits a **W013** warning naming the budget, usage,
  and limit. Informational only — evaluation continues.
- **Hard ceilings unchanged:** every X00x trip behaves identically in
  both modes. Data Mode never raises, disables, or softens a limit.
- **Audited raise:** raising a limit is an explicit operator act
  (`--max-*` flags, profile choice) and stays visible in the
  invocation — there is no silent or automatic raise path.
- Content-addressed values and partial evaluation exist in both modes
  (Phase CA/PE); Data Mode is where agents are *encouraged* to use
  them by default (agent-kit documents the default).

## 4. Diagnostics (M.005)

- **E024** (parse, error): bad mode marker — unknown string or
  non-string value, with the key's span.
- **W013** (eval, warning): approaching a budget ceiling under Data
  Mode. Value-free (counts and limits only).

## 5. Vectors (M.006)

Covered in-repo (`active-core/tests/mode.rs`): default Config,
data/config markers, E024 on unknown and non-string markers,
marker visibility in output, Config/Data tree identity, W013 at
80% with silence below, hard-ceiling trip in Data Mode. The
inspector exposes the mode (`"mode": "config" | "data"`).

**End of mode.md — M.001–M.007 complete.**
