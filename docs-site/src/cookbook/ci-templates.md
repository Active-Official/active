# CI templates: gate on Active, ship JSON

Copy-paste recipes. The first fails the build on diagnostics; the second
pre-evaluates Active to a JSON artifact for runtimes that must not embed
the evaluator.

## Gate job (GitHub Actions)

```yaml
jobs:
  active-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - name: Install Active CLI
        run: cargo install active-cli --locked
      - name: Check all configs (strict, warnings fail)
        run: |
          for f in $(git ls-files '*.active'); do
            active check "$f" || exit 1
          done
```

Strict is the default: warnings fail the run, no healing applies. Pin the
CLI version in real pipelines (`cargo install active-cli --version X.Y.Z
--locked`) so config gates never shift under you.

## Pre-evaluate to JSON (build-time artifact)

```yaml
      - name: Pre-evaluate configs to JSON
        run: |
          mkdir -p dist/config
          active eval service.active > dist/config/service.json
```

Ship `dist/config/*.json` with the release. Runtimes read plain JSON;
Active remains the source of truth, and the differential mode from the
migration kits proves the artifact matches the source in CI.

## Matrix pattern

Evaluate once per profile (`active eval --profile staging`, `--profile
prod`) and publish one artifact per environment. A failed evaluation
fails the build before anything deploys: no partial matrix, no silent
defaults beyond the `?:` defaults the file declares.
