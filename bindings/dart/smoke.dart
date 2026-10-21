// Dart smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
// package:ffi over the frozen C ABI (pubspec.yaml pins the dependency;
// run-smoke.py runs `dart pub get` first): opaque handles stay
// Pointer<Void>, every owned return is freed exactly once, borrowed code
// strings are copied with toDartString and never freed.
// Run: dart pub get && dart smoke.dart <libactive_ffi.so>
import 'dart:ffi' as ffi;
import 'dart:io' show exit, stderr;
import 'package:ffi/ffi.dart';

void check(bool cond, String msg) {
  if (!cond) {
    stderr.writeln('SMOKE-FAIL: $msg');
    exit(1);
  }
}

String ownedJson(ffi.Pointer<ffi.Void> val,
    ffi.Pointer<Utf8> Function(ffi.Pointer<ffi.Void>) toJson,
    void Function(ffi.Pointer<Utf8>) strFree) {
  final p = toJson(val);
  check(p != ffi.nullptr, 'to_json returned null');
  final s = p.toDartString();
  strFree(p);
  return s;
}

void main(List<String> args) {
  check(args.length == 1, 'usage: smoke.dart <libactive_ffi.so>');
  final lib = ffi.DynamicLibrary.open(args[0]);
  final abi =
      lib.lookupFunction<ffi.Uint32 Function(), int Function()>(
          'active_abi_version');
  final parse = lib.lookupFunction<
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<Utf8>, ffi.UintPtr, ffi.Pointer<ffi.Void>),
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<Utf8>, int, ffi.Pointer<ffi.Void>)>(
      'active_module_parse');
  final modFree = lib.lookupFunction<
      ffi.Void Function(ffi.Pointer<ffi.Void>),
      void Function(ffi.Pointer<ffi.Void>)>('active_module_free');
  final checkFn = lib.lookupFunction<
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<ffi.Void>, ffi.Pointer<ffi.Void>),
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<ffi.Void>, ffi.Pointer<ffi.Void>)>('active_check');
  final evalFn = lib.lookupFunction<
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<ffi.Void>, ffi.Pointer<ffi.Void>),
      ffi.Pointer<ffi.Void> Function(
          ffi.Pointer<ffi.Void>, ffi.Pointer<ffi.Void>)>('active_eval');
  final lenFn = lib.lookupFunction<
      ffi.UintPtr Function(ffi.Pointer<ffi.Void>),
      int Function(ffi.Pointer<ffi.Void>)>('active_diagnostics_len');
  final codeFn = lib.lookupFunction<
      ffi.Pointer<Utf8> Function(ffi.Pointer<ffi.Void>, ffi.UintPtr),
      ffi.Pointer<Utf8> Function(ffi.Pointer<ffi.Void>, int)>(
      'active_diagnostic_code');
  final setFree = lib.lookupFunction<
      ffi.Void Function(ffi.Pointer<ffi.Void>),
      void Function(ffi.Pointer<ffi.Void>)>('active_diagnostics_free');
  final valFree = lib.lookupFunction<
      ffi.Void Function(ffi.Pointer<ffi.Void>),
      void Function(ffi.Pointer<ffi.Void>)>('active_value_free');
  final toJson = lib.lookupFunction<
      ffi.Pointer<Utf8> Function(ffi.Pointer<ffi.Void>),
      ffi.Pointer<Utf8> Function(ffi.Pointer<ffi.Void>)>(
      'active_value_to_json');
  final strFree = lib.lookupFunction<
      ffi.Void Function(ffi.Pointer<Utf8>),
      void Function(ffi.Pointer<Utf8>)>('active_string_free');

  check(abi() == 1, 'ABI major mismatch');
  print('abi: 1 ok');

  void withModule(String src, void Function(ffi.Pointer<ffi.Void>) body) {
    final p = src.toNativeUtf8();
    // Pointer<Utf8>.length counts bytes to the NUL terminator, so it is
    // already the byte count (String.length would count UTF-16 units).
    final m = parse(p, p.length, ffi.nullptr);
    calloc.free(p);
    check(m != ffi.nullptr, 'parse returned null');
    try {
      body(m);
    } finally {
      modFree(m);
    }
  }

  withModule('port = 8080\n', (m) {
    final s = checkFn(m, ffi.nullptr);
    check(s != ffi.nullptr, 'check returned null');
    final n = lenFn(s);
    setFree(s);
    check(n == 0, 'expected clean');
  });
  print('check clean ok');

  withModule('port = 8080\n', (m) {
    final v = evalFn(m, ffi.nullptr);
    check(v != ffi.nullptr, 'eval returned null');
    final json = ownedJson(v, toJson, strFree);
    valFree(v);
    check(json.contains('8080'), 'value missing: $json');
  });
  print('eval port=8080 ok');

  withModule('timeout = 5s + 512MiB\n', (m) {
    final v = evalFn(m, ffi.nullptr);
    check(v == ffi.nullptr, 'units mixed evaluated clean');
    final s = checkFn(m, ffi.nullptr);
    check(s != ffi.nullptr, 'check returned null');
    final n = lenFn(s);
    final code = n > 0 ? codeFn(s, 0).toDartString() : '';
    setFree(s);
    check(n > 0, 'expected diagnostics');
    check(code == 'T001', 'expected T001, got $code');
  });
  print('units refused ok');

  withModule('token = secret("hunter2")\n', (m) {
    final v = evalFn(m, ffi.nullptr);
    check(v != ffi.nullptr, 'eval returned null');
    final json = ownedJson(v, toJson, strFree);
    valFree(v);
    check(json.contains('***SECRET***') && !json.contains('hunter2'),
        'redaction broken: $json');
  });
  print('secret redacted ok');
  print('SMOKE-OK');
}
