# Migration Kit C: AI agent and system-prompt configuration

A JSON blob with no schema, an out-of-range temperature, and a pasted
secret becomes typed, bounded, immutable-pinned, redacted Active. Files:
`kit-c-before.json` (the JSON you have) and `kit-c-after.active` (the
Active you want). The `.active` file is executed by the docs test suite,
so it can never rot.

## Step 1: see what the blob allows

```sh
active import -f json migration/kit-c-before.json
```

Import succeeds. Nothing checks that `temperature` 5.0 is absurd, that
`api_key` is live, or that `tools` contains only real tool names. Every
consumer re-validates by hand, or does not validate at all.

## Step 2: evaluate the after file

```sh
active check migration/kit-c-after.active
active eval migration/kit-c-after.active
```

`check` exits 0. `eval` prints `temperature` 0.7, `api_key` as
`***SECRET***`, and the pinned model. The `@ai_immutable` comment pins
`model_pin`; the `@intent` comment records why for humans and agents.

## Step 3: run the agent repair loop

Break the temperature, then repair by code plus span:

```sh
# temperature = 5.0 gives:
# [Error] T003: schema violation at 'agent.temperature': above maximum
# fix to 0.7, re-run active check, repeat until exit 0
```

Try to patch the pinned model and watch the immutability hold:

```text
@patch("model_pin", "acme-5")
# [Error] E019: patch error: patch target `model_pin` overlaps ai_immutable `model_pin`
```

Generate, check, repair by code plus span, re-evaluate. Timeout the loop
and cap repair iterations in automation; `check` exit 0 is the done signal.

## Silent-to-explicit checklist

| Before (silent) | After (explicit) |
|---|---|
| Untyped blob, any value passes | Closed schema: bounds, enum tools, required keys |
| Pasted secret travels in cleartext | `secret()` taints; exports redact by construction |
| Model pin edited by anyone | `@ai_immutable` rejects patches with E019 |
| Temperature 5.0 ships | T003 refuses above-maximum with an exact span |
| Intent lives in chat history | `@intent` travels with the binding |

## Differential mode (prove the migration in CI)

Import the JSON blob and compare against the Active evaluation (modulo
documented import normalization). The temperature and key differences are
the migration working as intended; anything else is a bug:

```sh
active import -f json kit-c-before.json > /tmp/blob-import.active
active eval /tmp/blob-import.active > /tmp/blob.json
active eval kit-c-after.active > /tmp/agent.json
# diff: temperature 5.0 vs 0.7 (bounded), api_key raw vs ***SECRET*** (taint)
```
