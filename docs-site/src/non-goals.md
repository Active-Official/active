# Non-goals

SOLID stays a configuration and data language. The following are
permanent non-goals, re-affirmed every major release cycle (checklist
C-070) to prevent scope creep:

- No Turing-complete computation (no loops, no recursion in the
  language itself; the dependency graph is deliberately a DAG).
- No arbitrary side effects (evaluation is hermetic).
- No dynamic code loading and no `eval` of strings.
- No unrestricted implicit coercion (the resilient coercion set is
  small, documented, and always reported).
- Never a general data-transformation language (generate *bindings*,
  not programs).

Any RFC proposing these is closed with a pointer here.
