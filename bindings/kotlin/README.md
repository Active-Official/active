# Kotlin Binding — JNI smoke over the frozen C ABI (CHECKLIST Phase 2 stretch)

`KActive.kt` plus a small JNI shim (`active_kni.c`, mirroring `bindings/java/active_jni.c`) drives `libactive_ffi`: opaque handles as `Long`, ABI major gate at startup, Allocator Model A (owned JSON freed inside the shim before crossing; modules freed via `Closeable.use`). No third-party dependencies. Build and run from the repo root:

```bash
gcc -shared -fPIC -I$JAVA_HOME/include -I$JAVA_HOME/include/linux -Iactive-ffi bindings/kotlin/active_kni.c -o libactive_kni.so path/to/libactive_ffi.a
kotlinc bindings/kotlin/KActive.kt -include-runtime -d kactive.jar
java -Djava.library.path=. -jar kactive.jar
```

Proven in a Linux container (kotlinc 1.3 on JDK 21, Linux staticlib):
ABI gate, clean check, eval value, T001 refusal, redaction with zero
raw leakage. `run-smoke.py` builds the shim and jar in a temp dir and
wraps the call for the shared harness, skipping cleanly without
kotlinc, java, gcc, or JNI headers.
