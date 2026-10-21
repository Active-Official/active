/* JNI shim for the Active Kotlin binding (CHECKLIST Phase 2 stretch).
 * Thin translation over the frozen C ABI mirroring bindings/java:
 * opaque handles as jlongs, no exceptions (null/empty signal refusal),
 * every owned pointer freed exactly once. Linked statically against
 * libactive_ffi.a so libactive_kni.so has no Active runtime dependency.
 */
#include <jni.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "active.h"

JNIEXPORT jboolean JNICALL Java_active_KActiveKt_checkAbi(JNIEnv *env,
                                                          jclass ignored) {
  (void)env;
  (void)ignored;
  return active_abi_version() == ACTIVE_ABI_VERSION ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jlong JNICALL Java_active_KActiveKt_nativeParse(JNIEnv *env,
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

JNIEXPORT void JNICALL Java_active_KActiveKt_nativeFreeModule(JNIEnv *env,
                                                              jclass ignored,
                                                              jlong h) {
  (void)env;
  (void)ignored;
  active_module_free((struct ActiveModule *)(uintptr_t)h);
}

JNIEXPORT jobjectArray JNICALL Java_active_KActiveKt_nativeCheck(JNIEnv *env,
                                                                 jclass ignored,
                                                                 jlong h) {
  (void)ignored;
  struct ActiveDiagnosticSet *set =
      active_check((const struct ActiveModule *)(uintptr_t)h, NULL);
  if (!set) {
    return NULL;
  }
  uintptr_t n = active_diagnostics_len(set);
  jclass str = (*env)->FindClass(env, "java/lang/String");
  if (str == NULL) {
    active_diagnostics_free(set);
    return NULL;
  }
  jobjectArray out = (*env)->NewObjectArray(env, (jsize)n, str, NULL);
  if (out == NULL) {
    active_diagnostics_free(set);
    return NULL;
  }
  for (uintptr_t i = 0; i < n; i++) {
    const char *code = active_diagnostic_code(set, i);
    jstring item = (*env)->NewStringUTF(env, code ? code : "");
    if (item == NULL) {
      active_diagnostics_free(set);
      return NULL;
    }
    (*env)->SetObjectArrayElement(env, out, (jsize)i, item);
    (*env)->DeleteLocalRef(env, item);
  }
  active_diagnostics_free(set);
  return out;
}

JNIEXPORT jstring JNICALL Java_active_KActiveKt_nativeEval(JNIEnv *env,
                                                           jclass ignored,
                                                           jlong h) {
  (void)ignored;
  struct ActiveValue *v =
      active_eval((const struct ActiveModule *)(uintptr_t)h, NULL);
  if (!v) {
    return NULL;
  }
  char *json = active_value_to_json(v);
  jstring out = (*env)->NewStringUTF(env, json ? json : "");
  active_string_free(json);
  active_value_free(v);
  return out;
}
