// Kotlin smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
// JNI over the frozen C ABI mirroring bindings/java: opaque handles as
// Longs, ABI major gate at startup, Allocator Model A (use{} frees every
// module; owned JSON freed inside the shim before crossing). No
// third-party dependencies.
// Build: kotlinc KActive.kt -include-runtime -d kactive.jar
// Run: java -Djava.library.path=<dir with libactive_kni.so> -jar kactive.jar
package active

private external fun checkAbi(): Boolean
private external fun nativeParse(src: ByteArray): Long
private external fun nativeFreeModule(h: Long)
private external fun nativeCheck(h: Long): Array<String>?
private external fun nativeEval(h: Long): String?

private class KModule(src: String) : java.io.Closeable {
    val handle: Long =
        nativeParse(src.toByteArray(Charsets.UTF_8)).also {
            check(it != 0L) { "parse returned null" }
        }
    override fun close() = nativeFreeModule(handle)
}

private fun check(cond: Boolean, msg: String) {
    if (!cond) {
        System.err.println("SMOKE-FAIL: $msg")
        kotlin.system.exitProcess(1)
    }
}

fun main() {
    System.loadLibrary("active_kni")
    check(checkAbi(), "ABI major mismatch")
    println("abi: 1 ok")

    KModule("port = 8080\n").use { m: KModule ->
        val codes = nativeCheck(m.handle) ?: error("check returned null")
        check(codes.isEmpty(), "expected clean")
    }
    println("check clean ok")

    KModule("port = 8080\n").use { m: KModule ->
        val json = nativeEval(m.handle)
        check(json != null, "eval returned null")
        check(json!!.contains("8080"), "value missing: $json")
    }
    println("eval port=8080 ok")

    KModule("timeout = 5s + 512MiB\n").use { m: KModule ->
        check(nativeEval(m.handle) == null, "units mixed evaluated clean")
        val codes = nativeCheck(m.handle) ?: error("check returned null")
        check(codes.isNotEmpty(), "expected diagnostics")
        check(codes[0] == "T001", "expected T001, got ${codes[0]}")
    }
    println("units refused ok")

    KModule("token = secret(\"hunter2\")\n").use { m: KModule ->
        val json = nativeEval(m.handle)
        check(json != null, "eval returned null")
        check(json!!.contains("***SECRET***") && !json!!.contains("hunter2"),
            "redaction broken: $json")
    }
    println("secret redacted ok")
    println("SMOKE-OK")
}
