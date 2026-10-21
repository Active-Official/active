# Java Binding — JNI smoke over the frozen C ABI (CHECKLIST 2.001)

`Active.java` exposes the agent loop surface (`check` returning
diagnostic codes, `eval` returning redacted JSON or throwing
`ActiveException` on refusal) over a JNI shim (`active_jni.c`) that is
a thin translation with no duplicated logic. Opaque handles travel as
longs; the static initializer gates on ABI major 1; `close()` frees
deterministically with a `Cleaner` backstop.

Proven in a Linux container (JDK 21, shim compiled with gcc against the
in-tree Linux staticlib): ABI gate, clean check, eval value, T001
refusal, secret redaction. `run-smoke.py` rebuilds everything from
source and runs the smoke; it skips cleanly without a JDK or C
compiler.
