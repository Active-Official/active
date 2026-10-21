/* JNI shim for the Active Java binding (CHECKLIST 2.001).
 * Thin translation over the frozen C ABI: no logic duplicated, every
 * owned handle freed exactly once on every path, including error paths.
 * Compiled against the in-tree active.h (which carries extern "C"
 * guards) and linked statically against libactive_ffi.a so the
 * resulting libactive_jni.so has no Active runtime dependency.
 * Generated headers come from `javac -h` (no hand-written signatures).
 */
#include <jni.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "active.h"

#define CLS "active/Active$ActiveException"

static void throw_active(JNIEnv *env, const char *msg) {
  jclass cls = (*env)->FindClass(env, CLS);
  if (cls == NULL) {
    /* No exception class available: clear the pending NoClassDefFoundError
       so the caller sees a clean null return, not a chained failure. */
    (*env)->ExceptionClear(env);
    return;
  }
  (*env)->ThrowNew(env, cls, msg);
}

JNIEXPORT void JNICALL Java_active_Active_checkAbi(JNIEnv *env,
                                                   jclass ignored) {
  (void)ignored;
  if (active_abi_version() != ACTIVE_ABI_VERSION) {
    throw_active(env, "ABI major mismatch: expected 1");
  }
}

JNIEXPORT jlong JNICALL Java_active_Active_nativeParse(JNIEnv *env,
                                                       jclass ignored,
                                                       jbyteArray src) {
  (void)ignored;
  jsize len = (*env)->GetArrayLength(env, src);
  jbyte *bytes = (*env)->GetByteArrayElements(env, src, NULL);
  if (bytes == NULL) {
    return 0;
  }
  struct ActiveModule *mod =
      active_module_parse((const uint8_t *)bytes, (uintptr_t)len, NULL);
  (*env)->ReleaseByteArrayElements(env, src, bytes, JNI_ABORT);
  return (jlong)(uintptr_t)mod;
}

JNIEXPORT void JNICALL Java_active_Active_nativeFreeModule(JNIEnv *env,
                                                           jclass ignored,
                                                           jlong h) {
  (void)env;
  (void)ignored;
  if (h != 0) {
    active_module_free((struct ActiveModule *)(uintptr_t)h);
  }
}

JNIEXPORT jobjectArray JNICALL Java_active_Active_nativeCheck(JNIEnv *env,
                                                              jclass ignored,
                                                              jlong h) {
  (void)ignored;
  jclass str_cls = (*env)->FindClass(env, "java/lang/String");
  jobjectArray empty = (*env)->NewObjectArray(env, 0, str_cls, NULL);
  if (empty == NULL) {
    return NULL;
  }
  if (h == 0) {
    return empty;
  }
  struct ActiveDiagnosticSet *set =
      active_check((const struct ActiveModule *)(uintptr_t)h, NULL);
  if (set == NULL) {
    return empty;
  }
  uintptr_t n = active_diagnostics_len(set);
  jobjectArray out = (*env)->NewObjectArray(env, (jsize)n, str_cls, NULL);
  if (out == NULL) {
    (*env)->DeleteLocalRef(env, empty);
    active_diagnostics_free(set);
    return NULL;
  }
  for (uintptr_t i = 0; i < n; i++) {
    const char *code = active_diagnostic_code(set, i);
    jstring s = (*env)->NewStringUTF(env, code != NULL ? code : "?");
    if (s == NULL) {
      active_diagnostics_free(set);
      return NULL;
    }
    (*env)->SetObjectArrayElement(env, out, (jsize)i, s);
    (*env)->DeleteLocalRef(env, s);
    if ((*env)->ExceptionCheck(env)) {
      active_diagnostics_free(set);
      return NULL;
    }
  }
  (*env)->DeleteLocalRef(env, empty);
  active_diagnostics_free(set);
  return out;
}

JNIEXPORT jstring JNICALL Java_active_Active_nativeEval(JNIEnv *env,
                                                        jclass ignored,
                                                        jlong h) {
  (void)ignored;
  if (h == 0) {
    return NULL;
  }
  struct ActiveValue *v =
      active_eval((const struct ActiveModule *)(uintptr_t)h, NULL);
  if (v == NULL) {
    return NULL;
  }
  char *json = active_value_to_json(v);
  active_value_free(v);
  if (json == NULL) {
    return NULL;
  }
  jstring out = (*env)->NewStringUTF(env, json);
  active_string_free(json);
  return out;
}
