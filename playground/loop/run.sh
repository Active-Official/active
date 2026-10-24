#!/usr/bin/env bash
# Playground repair loop (GAP-CLOSURE §2.10): generate -> check -> repair
# plan -> partial apply -> eval -> full repair, using the shipped CLI.
# Requires `active` on PATH or ACTIVE_BIN set.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="${ACTIVE_BIN:-active}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

source_doc="$here/broken.active"

echo "1. generate (jsonschema)"
"$bin" generate --from "$source_doc" --to jsonschema > "$work/schema.json"

echo "2. check (expected failure)"
if "$bin" check "$source_doc"; then
  echo "check unexpectedly passed" >&2
  exit 1
fi

echo "3. repair plan"
plan="$("$bin" repair --from "$source_doc" --format json)"
if [[ "$(printf '%s' "$plan" | grep -c '"kind"')" -lt 2 ]]; then
  echo "expected a multi-step plan" >&2
  exit 1
fi

echo "4. apply one step (partial repair)"
applied="$("$bin" repair --from "$source_doc" --apply-step 0 \
  --format json --output "$work/partial.active" 2>/dev/null)"
if ! cmp -s <(printf '%s' "$applied" | python3 -c \
  'import json,sys; print(json.load(sys.stdin)["repaired_source"], end="")') \
  "$work/partial.active"; then
  echo "repaired_source does not match the written file" >&2
  exit 1
fi
if "$bin" eval "$work/partial.active" >/dev/null 2>&1; then
  echo "one step removed every defect" >&2
  exit 1
fi

echo "5. apply the full plan"
"$bin" repair --from "$source_doc" --output "$work/repaired.active"

echo "6. eval the repaired document"
"$bin" eval "$work/repaired.active"
echo "loop complete"
