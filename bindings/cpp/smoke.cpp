// C++ smoke binding (CHECKLIST 2.003, PROPOSAL section 2.1).
// RAII over the frozen C ABI: opaque handles, ABI major gate at startup,
// Allocator Model A (unique_handle with custom deleters, exactly one
// free per owned pointer). Compile (Linux):
//   g++ -std=c++17 bindings/cpp/smoke.cpp -I active-ffi \
//     target/x86_64-unknown-linux-gnu/release/libactive_ffi.a \
//     -o /tmp/active-cpp-smoke && /tmp/active-cpp-smoke
// Run expects SMOKE-OK lines, exit 0 on success.
#include <cassert>
#include <cstring>
#include <iostream>
#include <memory>
#include <string>

#include "active.h"

namespace {
struct ModuleDeleter {
  void operator()(ActiveModule *h) const noexcept { active_module_free(h); }
};
struct SetDeleter {
  void operator()(ActiveDiagnosticSet *h) const noexcept {
    active_diagnostics_free(h);
  }
};
struct ValueDeleter {
  void operator()(ActiveValue *h) const noexcept { active_value_free(h); }
};
struct StringDeleter {
  void operator()(char *s) const noexcept { active_string_free(s); }
};
using Module = std::unique_ptr<ActiveModule, ModuleDeleter>;
using DiagSet = std::unique_ptr<ActiveDiagnosticSet, SetDeleter>;
using Value = std::unique_ptr<ActiveValue, ValueDeleter>;
using OwnedStr = std::unique_ptr<char, StringDeleter>;

Module parse(const std::string &src) {
  return Module(active_module_parse(
      reinterpret_cast<const uint8_t *>(src.data()), src.size(), nullptr));
}

std::string to_json(const ActiveValue *v) {
  OwnedStr s(active_value_to_json(v));
  return s ? std::string(s.get()) : std::string();
}

#define CHECK(cond, msg)                                                       \
  do {                                                                         \
    if (!(cond)) {                                                             \
      std::cerr << "SMOKE-FAIL: " << msg << "\n";                              \
      return 1;                                                                \
    }                                                                          \
  } while (0)

int run() {
  CHECK(active_abi_version() == ACTIVE_ABI_VERSION, "ABI major mismatch");
  std::cout << "abi: 1 ok\n";
  {
    auto mod = parse("port = 8080\n");
    CHECK(mod != nullptr, "parse returned null");
    DiagSet set(active_check(mod.get(), nullptr));
    CHECK(set != nullptr, "check returned null");
    CHECK(active_diagnostics_len(set.get()) == 0, "expected clean");
  }
  std::cout << "check clean ok\n";
  {
    auto mod = parse("port = 8080\n");
    CHECK(mod != nullptr, "parse returned null");
    Value v(active_eval(mod.get(), nullptr));
    CHECK(v != nullptr, "eval returned null");
    CHECK(to_json(v.get()).find("8080") != std::string::npos,
          "value missing 8080");
  }
  std::cout << "eval port=8080 ok\n";
  {
    auto mod = parse("timeout = 5s + 512MiB\n");
    CHECK(mod != nullptr, "parse returned null");
    Value v(active_eval(mod.get(), nullptr));
    CHECK(v == nullptr, "units mixed evaluated clean");
    DiagSet set(active_check(mod.get(), nullptr));
    CHECK(set != nullptr, "check returned null");
    const char *code = active_diagnostic_code(set.get(), 0);
    CHECK(code != nullptr && std::string(code) == "T001", "expected T001");
  }
  std::cout << "units refused ok\n";
  {
    auto mod = parse("token = secret(\"hunter2\")\n");
    CHECK(mod != nullptr, "parse returned null");
    Value v(active_eval(mod.get(), nullptr));
    CHECK(v != nullptr, "eval returned null");
    std::string json = to_json(v.get());
    CHECK(json.find("***SECRET***") != std::string::npos &&
              json.find("hunter2") == std::string::npos,
          "redaction broken: " + json);
  }
  std::cout << "secret redacted ok\n";
  std::cout << "SMOKE-OK\n";
  return 0;
}
} // namespace

int main() {
  try {
    return run();
  } catch (const std::exception &e) {
    std::cerr << "SMOKE-FAIL: exception: " << e.what() << "\n";
    return 1;
  }
}
