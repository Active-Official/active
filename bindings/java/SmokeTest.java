// Java smoke test (CHECKLIST 2.001): ABI gate, clean check, eval value,
// T001 refusal, secret redaction. Run from bindings/java with the
// libactive_jni.so built (see README). Prints TEST-OK, exit 0 on success.
import active.Active;

public class SmokeTest {
    static void check(boolean cond, String what) {
        if (!cond) {
            System.err.println("SMOKE-FAIL: " + what);
            System.exit(1);
        }
    }

    public static void main(String[] args) {
        try (Active m = new Active("port = 8080\n")) {
            check(m.check().length == 0, "expected clean");
            System.out.println("check clean ok");
            String json = m.eval();
            check(json.contains("8080"), "value missing 8080: " + json);
            System.out.println("eval port=8080 ok");
        }
        try (Active m = new Active("timeout = 5s + 512MiB\n")) {
            String[] codes = m.check();
            check(codes.length > 0 && codes[0].equals("T001"),
                    "expected T001, got " + String.join(",", codes));
            boolean refused = false;
            try {
                m.eval();
            } catch (Active.ActiveException e) {
                refused = true;
            }
            check(refused, "units mixed evaluated clean");
            System.out.println("units refused ok");
        }
        try (Active m = new Active("token = secret(\"hunter2\")\n")) {
            String json = m.eval();
            check(json.contains("***SECRET***") && !json.contains("hunter2"),
                    "redaction broken: " + json);
            System.out.println("secret redacted ok");
        }
        System.out.println("SMOKE-OK");
    }
}
