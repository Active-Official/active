# Deprecation policy (G-011b, G-025)

Diagnostic codes are API: agents switch on them, so they version like API.

1. Codes are never reused. A retired meaning keeps its code retired forever.
2. Change within 1.x is additive only: new codes appear, old codes keep
   their meaning. Anything else waits for a major.
3. ABI major bumps ride `ACTIVE_ABI_VERSION`; bindings fail closed on
   mismatch (H-002). Minor ABI additions are additive functions plus docs.
4. Deprecating a code or entry needs: announced replacement, one minor
   release of overlap with both working, and a changelog entry naming the
   removal version. Removal happens at a major at the earliest.
5. The conformance manifest pins the code set under test; a deprecation
   updates the manifest the same release it ships.
