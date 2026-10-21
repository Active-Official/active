<?php
// PHP smoke binding (CHECKLIST 2.006, PROPOSAL section 2.1).
// FFI over the frozen C ABI (needs ext-ffi enabled): opaque pointers as
// FFI\CData, ABI major gate at load, Allocator Model A (every owned
// pointer freed; a PHP shutdown function backstops the module handle).
// Run: php -d ffi.enable=1 bindings/php/smoke.php /path/to/libactive_ffi.so

declare(strict_types=1);

if ($argc < 2) { fwrite(STDERR, "usage: smoke.php <libactive_ffi.so>\n"); exit(2); }
if (!extension_loaded("ffi")) { fwrite(STDERR, "SKIP: ext-ffi not available\n"); exit(0); }

$ffi = FFI::cdef("
typedef struct ActiveModule ActiveModule;
typedef struct ActiveDiagnosticSet ActiveDiagnosticSet;
typedef struct ActiveValue ActiveValue;
typedef struct ActiveBudget { uint32_t struct_size; uint64_t max_nodes; uint64_t max_depth; uint64_t max_array_len; uint64_t max_keys; uint64_t max_wall_ms; uint64_t max_value_size; uint64_t max_total_value_size; } ActiveBudget;
uint32_t active_abi_version(void);
ActiveModule* active_module_parse(const uint8_t* source, uintptr_t len, const void* options);
void active_module_free(ActiveModule* handle);
ActiveDiagnosticSet* active_check(const ActiveModule* module, const ActiveBudget* budget);
uintptr_t active_diagnostics_len(const ActiveDiagnosticSet* set);
const char* active_diagnostic_code(const ActiveDiagnosticSet* set, uintptr_t index);
void active_diagnostics_free(ActiveDiagnosticSet* set);
ActiveValue* active_eval(const ActiveModule* module, const ActiveBudget* budget);
char* active_value_to_json(const ActiveValue* value);
void active_value_free(ActiveValue* value);
void active_string_free(char* s);
", $argv[1]);

function fail(string $msg): never { fwrite(STDERR, "SMOKE-FAIL: $msg\n"); exit(1); }
function check(bool $cond, string $msg): void { if (!$cond) fail($msg); }
function cstr($ptr): ?string {
    if ($ptr === null) return null;
    // No FFI::string() on PHP < 8.4: measure with libc strlen, then read.
    static $strlen = null;
    if ($strlen === null) {
        $strlen = FFI::cdef("size_t strlen(const char *s);", "libc.so.6");
    }
    $len = $strlen->strlen($ptr);
    $out = "";
    for ($i = 0; $i < $len; $i++) {
        $b = $ptr[$i];
        $out .= chr(is_int($b) ? $b : ord($b));
    }
    return $out;
}

if ($ffi->active_abi_version() !== 1) fail("ABI major mismatch");
echo "abi: 1 ok\n";

function withModule(string $src, callable $fn): void {
    global $ffi;
    $buf = FFI::new("uint8_t[" . strlen($src) . "]", false);
    FFI::memcpy($buf, $src, strlen($src));
    $mod = $ffi->active_module_parse($buf, strlen($src), null);
    if ($mod === null) fail("parse returned null");
    try { $fn($mod); } finally { $ffi->active_module_free($mod); }
}

withModule("port = 8080\n", function ($mod) use ($ffi) {
    $set = $ffi->active_check($mod, null);
    check($set !== null, "check returned null");
    try { check($ffi->active_diagnostics_len($set) === 0, "expected clean"); }
    finally { $ffi->active_diagnostics_free($set); }
});
echo "check clean ok\n";

withModule("port = 8080\n", function ($mod) use ($ffi) {
    $val = $ffi->active_eval($mod, null);
    check($val !== null, "eval returned null");
    try {
        $json = $ffi->active_value_to_json($val);
        check($json !== null, "to_json returned null");
        try {
            $s = cstr($json);
            check($s !== null && str_contains($s, "8080"), "value missing: $s");
        } finally { $ffi->active_string_free($json); }
    } finally { $ffi->active_value_free($val); }
});
echo "eval port=8080 ok\n";

withModule("timeout = 5s + 512MiB\n", function ($mod) use ($ffi) {
    $val = $ffi->active_eval($mod, null);
    check($val === null, "units mixed evaluated clean");
    $set = $ffi->active_check($mod, null);
    check($set !== null, "check returned null");
    try {
        check($ffi->active_diagnostics_len($set) > 0, "expected diagnostics");
        check(cstr($ffi->active_diagnostic_code($set, 0)) === "T001", "expected T001");
    } finally { $ffi->active_diagnostics_free($set); }
});
echo "units refused ok\n";

withModule("token = secret(\"hunter2\")\n", function ($mod) use ($ffi) {
    $val = $ffi->active_eval($mod, null);
    check($val !== null, "eval returned null");
    try {
        $json = $ffi->active_value_to_json($val);
        check($json !== null, "to_json returned null");
        try {
            $s = cstr($json);
            check($s !== null && str_contains($s, "***SECRET***") && !str_contains($s, "hunter2"), "redaction broken: $s");
        } finally { $ffi->active_string_free($json); }
    } finally { $ffi->active_value_free($val); }
});
echo "secret redacted ok\n";
echo "SMOKE-OK\n";
