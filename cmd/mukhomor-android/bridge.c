// The Java boundary uses UTF-8 byte arrays, never JNI's modified UTF-8.
#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include "_cgo_export.h"

static JavaVM *vm;
static jclass bridge;
static jmethodID protect_method;

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *java_vm, void *reserved) {
    (void)reserved;
    JNIEnv *env = NULL;
    if ((*java_vm)->GetEnv(java_vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
    jclass local = (*env)->FindClass(env, "org/mukhomor/app/NativeBridge");
    if (!local) return JNI_ERR;
    bridge = (*env)->NewGlobalRef(env, local);
    (*env)->DeleteLocalRef(env, local);
    protect_method = (*env)->GetStaticMethodID(env, bridge, "protect", "(I)Z");
    if (!protect_method) return JNI_ERR;
    vm = java_vm;
    return JNI_VERSION_1_6;
}

int protect_socket(int fd) {
    if (!vm) return 0;
    JNIEnv *env = NULL;
    int attached = 0;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) {
        if ((*vm)->AttachCurrentThread(vm, &env, NULL) != JNI_OK) return 0;
        attached = 1;
    }
    jboolean result = (*env)->CallStaticBooleanMethod(env, bridge, protect_method, (jint)fd);
    if ((*env)->ExceptionCheck(env)) { (*env)->ExceptionClear(env); result = JNI_FALSE; }
    if (attached) (*vm)->DetachCurrentThread(vm);
    return result == JNI_TRUE;
}

static char *copy_bytes(JNIEnv *env, jbyteArray value, int max) {
    if (!value) return NULL;
    jsize size = (*env)->GetArrayLength(env, value);
    if (size < 0 || size > max) return NULL;
    char *out = malloc((size_t)size + 1);
    if (!out) return NULL;
    (*env)->GetByteArrayRegion(env, value, 0, size, (jbyte *)out);
    out[size] = 0;
    if ((*env)->ExceptionCheck(env) || memchr(out, 0, size)) { free(out); return NULL; }
    return out;
}

static jbyteArray result_bytes(JNIEnv *env, char *text) {
    if (!text) return NULL;
    size_t len = strlen(text);
    jbyteArray result = (*env)->NewByteArray(env, (jsize)len);
    if (result) (*env)->SetByteArrayRegion(env, result, 0, (jsize)len, (jbyte *)text);
    mukhomor_free(text);
    return result;
}

JNIEXPORT jbyteArray JNICALL Java_org_mukhomor_app_NativeBridge_nativeRequest(JNIEnv *env, jclass cls, jbyteArray root, jbyteArray request, jbyteArray torPath, jint fd) {
    (void)cls;
    char *r=copy_bytes(env, root, 4096), *q=copy_bytes(env, request, 4*1024*1024);
    if (!r || !q) { free(r); free(q); return NULL; }
    char *t=copy_bytes(env,torPath,4096);
    if(!t){free(r);free(q);return NULL;}
    char *out=mukhomor_request(r,q,t,fd);
    free(t);
    free(r); free(q);
    return result_bytes(env,out);
}

JNIEXPORT jbyteArray JNICALL Java_org_mukhomor_app_NativeBridge_nativePackages(JNIEnv *env, jclass cls) {
    (void)cls; return result_bytes(env,mukhomor_packages());
}
