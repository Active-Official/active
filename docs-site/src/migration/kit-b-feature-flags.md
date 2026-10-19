# Migration Kit B: feature flags and multi-environment overlays

Two YAML files that drifted apart become one Active source with profiles.
Files: `kit-b-before-staging.yaml` and `kit-b-before-prod.yaml` (the YAML
you have) and `kit-b-after.active` (the Active you want). The `.active`
file is executed by the docs test suite, so it can never rot.

## Step 1: find the drift

```sh
diff kit-b-before-staging.yaml kit-b-before-prod.yaml
```

`dark_launch` is `false` in staging and `true` in production. Nothing in
YAML reports that disagreement; diffing files is the entire review process,
and it does not scale past two environments.

## Step 2: evaluate every environment from one source

```sh
active eval kit-b-after.active
active eval --profile staging kit-b-after.active
active eval --profile prod kit-b-after.active
```

Default gives `rollout_pct` 25, staging 100, prod 10, all with
`dark_launch` false. One file, three environments, no drift possible.

## Step 3: see the blast radius before you change a key

```sh
active impact flags.rollout_pct kit-b-after.active
```

```text
flags.effective
```

`effective` derives from `rollout_pct`, so the graph names it as the
dependent. Know this semantic (spec Chapter 3): profiles merge evaluated
values, scalars last-win, so a derived value keeps its base evaluation
while the overridden key changes. Derived values document data flow for
impact analysis; per-environment truth lives in the profile blocks.

## Step 4: break it on purpose

```sh
active eval --profile prod kit-b-after.active
```

Change `dark_launch` to `true` inside the prod profile and re-run: the
value flips with no other file touched. Change `rollout_pct` to `-5` and
`active check` fails with the schema minimum diagnostic at an exact span.
Delete a profile and the base values still evaluate.

## Silent-to-explicit checklist

| Before (silent) | After (explicit) |
|---|---|
| One file per environment, drift invisible | One source plus profiles; environments differ only where written |
| Anchor merges silently win | Overlapping an `@ai_immutable` path errors E019; profile scalars last-win by rule |
| No one knows what a key affects | `active impact` names dependents |
| Staging-only flags leak to prod by copy | Closed review: diff shows profile blocks only |

## Differential mode (prove the migration in CI)

Import both YAML files and compare each against the matching profile
evaluation (modulo documented import normalization). Drift between the old
files becomes a visible diff against one source instead of a silent pair:

```sh
active import -f yaml kit-b-before-prod.yaml > /tmp/prod-import.active
active eval /tmp/prod-import.active > /tmp/prod-import.json
active eval --profile prod kit-b-after.active > /tmp/prod.json
# diff: dark_launch true (old drift) vs false (new source of truth)
```
