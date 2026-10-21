// Swift smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
// C shim (active_swift.c) over the frozen C ABI via a generated
// modulemap (run-smoke.py writes it with the real include path): opaque
// handles stay UnsafeMutableRawPointer, every owned return is freed
// exactly once, borrowed code strings are copied with String(cString:)
// and never freed.
// Build: swiftc -I <modulemap dir> smoke.swift active_swift.c <lib>
import ActiveShim
import Foundation

func check(_ cond: Bool, _ msg: String) {
    if !cond {
        fputs("SMOKE-FAIL: \(msg)\n", stderr)
        exit(1)
    }
}

func withModule(_ src: String, _ body: (UnsafeMutableRawPointer) -> Void) {
    let bytes = Array(src.utf8)
    let mod: UnsafeMutableRawPointer? = bytes.withUnsafeBufferPointer { buf in
        sw_parse(buf.baseAddress, UInt(buf.count))
            .map { UnsafeMutableRawPointer($0) }
    }
    check(mod != nil, "parse returned null")
    defer { sw_mod_free(mod!) }
    body(mod!)
}

func ownedJson(_ val: UnsafeMutableRawPointer) -> String {
    guard let p = sw_to_json(val) else {
        check(false, "to_json returned null")
        return ""
    }
    let s = String(cString: p)
    sw_str_free(p)
    return s
}

check(sw_abi() == 1, "ABI major mismatch")
print("abi: 1 ok")

withModule("port = 8080\n") { m in
    guard let set = sw_check(m) else {
        check(false, "check returned null")
        return
    }
    let n = sw_len(set)
    sw_set_free(set)
    check(n == 0, "expected clean")
}
print("check clean ok")

withModule("port = 8080\n") { m in
    guard let v = sw_eval(m).map({ UnsafeMutableRawPointer($0) }) else {
        check(false, "eval returned null")
        return
    }
    let json = ownedJson(v)
    sw_val_free(v)
    check(json.contains("8080"), "value missing: \(json)")
}
print("eval port=8080 ok")

withModule("timeout = 5s + 512MiB\n") { m in
    check(sw_eval(m) == nil, "units mixed evaluated clean")
    guard let set = sw_check(m) else {
        check(false, "check returned null")
        return
    }
    let n = sw_len(set)
    var code = ""
    if n > 0 {
        if let p = sw_code(set, 0) {
            code = String(cString: p)
        }
    }
    sw_set_free(set)
    check(n > 0, "expected diagnostics")
    check(code == "T001", "expected T001, got \(code)")
}
print("units refused ok")

withModule("token = secret(\"hunter2\")\n") { m in
    guard let v = sw_eval(m).map({ UnsafeMutableRawPointer($0) }) else {
        check(false, "eval returned null")
        return
    }
    let json = ownedJson(v)
    sw_val_free(v)
    check(json.contains("***SECRET***") && !json.contains("hunter2"),
          "redaction broken: \(json)")
}
print("secret redacted ok")
print("SMOKE-OK")
