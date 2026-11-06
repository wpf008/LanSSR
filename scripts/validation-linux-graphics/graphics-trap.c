/* Test-only dynamic API interception. glutin resolves function pointers with
 * dlsym(handle), so ordinary LD_PRELOAD symbol definitions are insufficient.
 * The real dlsym is obtained through glibc's versioned dlvsym entry point. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef void *(*dlsym_fn)(void *, const char *);
typedef int (*glx_query_fn)(void *, int *, int *);
typedef unsigned int (*egl_initialize_fn)(void *, int *, int *);
typedef unsigned int (*egl_get_error_fn)(void);

static glx_query_fn real_glx_extension;
static glx_query_fn real_glx_version;
static egl_initialize_fn real_egl_initialize;
static egl_get_error_fn real_egl_get_error;
static _Thread_local unsigned int injected_egl_error;

static dlsym_fn original_dlsym(void) {
#if defined(__aarch64__)
    return (dlsym_fn)dlvsym(RTLD_NEXT, "dlsym", "GLIBC_2.17");
#else
    return (dlsym_fn)dlvsym(RTLD_NEXT, "dlsym", "GLIBC_2.2.5");
#endif
}

static void record(const char *message) {
    const char *path = getenv("LANSSR_GRAPHICS_TRACE");
    if (!path || !*path) return;
    int file = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (file >= 0) {
        (void)write(file, message, strlen(message));
        (void)write(file, "\n", 1);
        close(file);
    }
}

static int trap_glx(void) {
    const char *mode = getenv("LANSSR_GRAPHICS_TRAP");
    return mode && !strcmp(mode, "broken-glx");
}

static int wrapped_glx_extension(void *display, int *error, int *event) {
    record("CALL glXQueryExtension");
    if (trap_glx()) _exit(86);
    return real_glx_extension(display, error, event);
}

static int wrapped_glx_version(void *display, int *major, int *minor) {
    record("CALL glXQueryVersion");
    if (trap_glx()) _exit(86);
    return real_glx_version(display, major, minor);
}

static unsigned int wrapped_egl_initialize(void *display, int *major, int *minor) {
    const char *mode = getenv("LANSSR_GRAPHICS_TRAP");
    record("CALL eglInitialize");
    if (mode && !strcmp(mode, "unavailable-egl")) {
        /* EGL_NOT_INITIALIZED. A failed EGL entry point must also set its
         * per-thread error, just as an actual unavailable implementation. */
        injected_egl_error = 0x3001;
        return 0;
    }
    return real_egl_initialize(display, major, minor);
}

static unsigned int wrapped_egl_get_error(void) {
    if (injected_egl_error) {
        unsigned int error = injected_egl_error;
        injected_egl_error = 0;
        return error;
    }
    return real_egl_get_error();
}

void *dlsym(void *handle, const char *symbol) {
    dlsym_fn lookup = original_dlsym();
    if (!lookup) _exit(87);
    void *address = lookup(handle, symbol);
    if (!address) return address;
    if (!strcmp(symbol, "glXQueryExtension")) {
        real_glx_extension = (glx_query_fn)address;
        return (void *)wrapped_glx_extension;
    }
    if (!strcmp(symbol, "glXQueryVersion")) {
        real_glx_version = (glx_query_fn)address;
        return (void *)wrapped_glx_version;
    }
    if (!strcmp(symbol, "eglInitialize")) {
        real_egl_initialize = (egl_initialize_fn)address;
        return (void *)wrapped_egl_initialize;
    }
    if (!strcmp(symbol, "eglGetError")) {
        real_egl_get_error = (egl_get_error_fn)address;
        return (void *)wrapped_egl_get_error;
    }
    return address;
}
