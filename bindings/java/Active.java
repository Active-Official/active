// Java binding (CHECKLIST 2.001, PROPOSAL section 2.1).
// JNI over the frozen C ABI: opaque handles as longs, ABI major gate at
// class load, Allocator Model A (every owned pointer freed exactly once,
// close() plus Cleaner fallback). No third-party dependencies.
package active;

import java.lang.ref.Cleaner;
import java.nio.charset.StandardCharsets;

public final class Active implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    static {
        System.loadLibrary("active_jni");
        checkAbi();
    }

    private static native void checkAbi();
    private static native long nativeParse(byte[] src);
    private static native void nativeFreeModule(long h);
    private static native String[] nativeCheck(long h);
    private static native String nativeEval(long h);

    private final long handle;
    @SuppressWarnings("unused")
    private final Cleaner.Cleanable cleanable;
    private boolean closed;

    public Active(String source) {
        byte[] bytes = source.getBytes(StandardCharsets.UTF_8);
        long h = nativeParse(bytes);
        if (h == 0) throw new ActiveException("parse returned null");
        this.handle = h;
        final long fh = h;
        this.cleanable =
                CLEANER.register(this, () -> nativeFreeModule(fh));
    }

    /** Diagnostic codes; empty means clean. Serialized with close(). */
    public synchronized String[] check() {
        ensureOpen();
        return nativeCheck(handle);
    }

    /** Redacted JSON value; throws ActiveException on refusal. Serialized with close(). */
    public synchronized String eval() {
        ensureOpen();
        String out = nativeEval(handle);
        if (out == null) {
            String[] codes = nativeCheck(handle);
            throw new ActiveException(
                    "refused: " + (codes.length > 0 ? codes[0] : "unknown"));
        }
        return out;
    }

    private void ensureOpen() {
        if (closed) throw new IllegalStateException("module is closed");
    }

    @Override
    public void close() {
        if (!closed) {
            closed = true;
            cleanable.clean();
        }
    }

    public static final class ActiveException extends RuntimeException {
        public ActiveException(String message) {
            super(message);
        }
    }
}
