# Security Policy (C-064)

## What this covers

The SOLID reference implementation in this repository (parser,
evaluator, CLI, formatter, language server, converters), the
standard schema library (`schemas/`), and the release artifacts
(binaries, checksums, attestations, signatures).

## Reporting a vulnerability

**Do not open a public issue.** Report privately through a
[GitHub private security advisory](https://github.com/RatioArtificiosa/solid-lang/security/advisories/new)
against `RatioArtificiosa/solid-lang`.

Include: affected versions / commits, a minimal `.solid` reproducer
(or the attack scenario for supply-chain issues), and the impact
you see (secret exfiltration, evaluation escape, signature bypass,
redaction failure, non-determinism).

## Response commitments

The repository owner (see `.github/CODEOWNERS`) targets these
timelines; reports wait in the private advisory queue, so a deputy
with repo security access can pick them up if the owner is away:

- Acknowledgement within **3 days**.
- Severity assessment (critical / high / medium / low) with a
  remediation target: critical **14 days**, high **30 days**,
  medium/low next minor release.
- Fixes ship with a regression test, a CHANGELOG entry, and credit
  to the reporter (or anonymity on request). Coordinated
  disclosure: details stay private until the fix is released.

## Supported versions

| Version | Supported |
|---|---|
| 1.x (once declared, C-090) | Full support |
| 0.1.0-alpha.x | Best effort; upgrade to 1.x when available |

## Scope notes

- Secret handling is taint-as-kind: reports of raw secret material
  in exports, diagnostics, logs, errors, hover text, graphs, or
  generated docs are treated as **critical**.
- Evaluation-escape reports (I/O, shell, network from eval;
  unbounded expansion past budgets) are treated as **critical**.
- The threat model (`spec/threat-model.md`) defines what is
  defended; items it documents as accepted residual risk (e.g.
  secrets authored in *plain* strings) are presumed not
  vulnerabilities absent a new exfiltration vector — but they are
  valid discussion topics, and a novel vector reopens them.
- Dependencies: CI runs `cargo audit` on every push; reports
  against our lockfile are triaged there and forwarded upstream
  when the flaw is not ours.
