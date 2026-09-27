/*
 * Comrade WebKit path shim (rootless Linux).
 *
 * Problem: distro WebKitGTK bakes its helper-process directory
 * (/usr/lib/webkit2gtk-4.1) in at compile time. Without root we vendor the
 * package into ~/comrade-sysroot instead, so every absolute lookup under the
 * baked prefix is rewritten to the vendored copy.
 *
 * Activated by re-execing the app with LD_PRELOAD pointed here plus
 * COMRADE_WEBKIT_DIR set (see src-tauri/src/main.rs `ensure_webkit_paths`).
 * Interception happens in the UI process, which is the side that spawns
 * WebKitNetworkProcess / WebKitWebProcess / WebKitGPUProcess and dlopens
 * the injected bundle — children inherit the rewritten paths naturally.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define SYS_PREFIX "/usr/lib/webkit2gtk-4.1"
#define SYS_PREFIX_LEN (sizeof(SYS_PREFIX) - 1)

static char g_base[4096] = "";
static size_t g_base_len = 0;

__attribute__((constructor)) static void shim_init(void) {
    const char *env = getenv("COMRADE_WEBKIT_DIR");
    if (env && env[0]) {
        snprintf(g_base, sizeof(g_base), "%s", env);
    } else {
        const char *home = getenv("HOME");
        if (!home)
            home = "";
        snprintf(g_base, sizeof(g_base), "%s/comrade-sysroot/usr/lib/webkit2gtk-4.1", home);
    }
    g_base_len = strlen(g_base);
}

/* Thread-local rewrite buffer. Returns the original pointer when no
 * rewrite applies, so callers never need to free. */
static _Thread_local char t_buf[8192];

static const char *redirect_path(const char *path) {
    if (!path || !g_base_len)
        return path;
    if (strncmp(path, SYS_PREFIX, SYS_PREFIX_LEN) != 0)
        return path;
    char next = path[SYS_PREFIX_LEN];
    if (next != '\0' && next != '/')
        return path;
    size_t rest_len = strlen(path + SYS_PREFIX_LEN);
    if (g_base_len + rest_len + 1 > sizeof(t_buf))
        return path;
    memcpy(t_buf, g_base, g_base_len);
    memcpy(t_buf + g_base_len, path + SYS_PREFIX_LEN, rest_len + 1);
    return t_buf;
}

/* ---- file access ---- */

int open(const char *path, int flags, ...) {
    static int (*real_open)(const char *, int, ...) = NULL;
    if (!real_open)
        real_open = dlsym(RTLD_NEXT, "open");
    mode_t mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, mode_t);
        va_end(ap);
    }
    return real_open(redirect_path(path), flags, mode);
}

int open64(const char *path, int flags, ...) {
    static int (*real_open64)(const char *, int, ...) = NULL;
    if (!real_open64)
        real_open64 = dlsym(RTLD_NEXT, "open64");
    mode_t mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, mode_t);
        va_end(ap);
    }
    return real_open64(redirect_path(path), flags, mode);
}

int openat(int dirfd, const char *path, int flags, ...) {
    static int (*real_openat)(int, const char *, int, ...) = NULL;
    if (!real_openat)
        real_openat = dlsym(RTLD_NEXT, "openat");
    mode_t mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, mode_t);
        va_end(ap);
    }
    return real_openat(dirfd, redirect_path(path), flags, mode);
}

int openat64(int dirfd, const char *path, int flags, ...) {
    static int (*real_openat64)(int, const char *, int, ...) = NULL;
    if (!real_openat64)
        real_openat64 = dlsym(RTLD_NEXT, "openat64");
    mode_t mode = 0;
    if (flags & (O_CREAT | O_TMPFILE)) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, mode_t);
        va_end(ap);
    }
    return real_openat64(dirfd, redirect_path(path), flags, mode);
}

int access(const char *path, int mode) {
    static int (*real_access)(const char *, int) = NULL;
    if (!real_access)
        real_access = dlsym(RTLD_NEXT, "access");
    return real_access(redirect_path(path), mode);
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    static int (*real_faccessat)(int, const char *, int, int) = NULL;
    if (!real_faccessat)
        real_faccessat = dlsym(RTLD_NEXT, "faccessat");
    return real_faccessat(dirfd, redirect_path(path), mode, flags);
}

int stat(const char *path, struct stat *buf) {
    static int (*real_stat)(const char *, struct stat *) = NULL;
    if (!real_stat)
        real_stat = dlsym(RTLD_NEXT, "stat");
    return real_stat(redirect_path(path), buf);
}

int lstat(const char *path, struct stat *buf) {
    static int (*real_lstat)(const char *, struct stat *) = NULL;
    if (!real_lstat)
        real_lstat = dlsym(RTLD_NEXT, "lstat");
    return real_lstat(redirect_path(path), buf);
}

int fstatat(int dirfd, const char *path, struct stat *buf, int flags) {
    static int (*real_fstatat)(int, const char *, struct stat *, int) = NULL;
    if (!real_fstatat)
        real_fstatat = dlsym(RTLD_NEXT, "fstatat");
    return real_fstatat(dirfd, redirect_path(path), buf, flags);
}

/* dlopen goes through ld.so's internal open — invisible to the open() hooks
 * above — so intercept it directly as well (covers g_module_open). */
void *dlopen(const char *filename, int flags) {
    static void *(*real_dlopen)(const char *, int) = NULL;
    if (!real_dlopen)
        real_dlopen = dlsym(RTLD_NEXT, "dlopen");
    return real_dlopen(filename ? redirect_path(filename) : filename, flags);
}

void *dlmopen(long nsid, const char *filename, int flags) {
    static void *(*real_dlmopen)(long, const char *, int) = NULL;
    if (!real_dlmopen)
        real_dlmopen = dlsym(RTLD_NEXT, "dlmopen");
    return real_dlmopen(nsid, filename ? redirect_path(filename) : filename, flags);
}

/* ---- process spawning (GLib spawns helpers via posix_spawn or fork+exec) ---- */

int execve(const char *path, char *const argv[], char *const envp[]) {
    static int (*real_execve)(const char *, char *const[], char *const[]) = NULL;
    if (!real_execve)
        real_execve = dlsym(RTLD_NEXT, "execve");
    return real_execve(redirect_path(path), argv, envp);
}

int execv(const char *path, char *const argv[]) {
    static int (*real_execv)(const char *, char *const[]) = NULL;
    if (!real_execv)
        real_execv = dlsym(RTLD_NEXT, "execv");
    return real_execv(redirect_path(path), argv);
}

int execvp(const char *file, char *const argv[]) {
    static int (*real_execvp)(const char *, char *const[]) = NULL;
    if (!real_execvp)
        real_execvp = dlsym(RTLD_NEXT, "execvp");
    return real_execvp(redirect_path(file), argv);
}

int posix_spawn(pid_t *pid, const char *path,
                const posix_spawn_file_actions_t *actions,
                const posix_spawnattr_t *attrp,
                char *const argv[], char *const envp[]) {
    static int (*real_spawn)(pid_t *, const char *,
                             const posix_spawn_file_actions_t *,
                             const posix_spawnattr_t *,
                             char *const[], char *const[]) = NULL;
    if (!real_spawn)
        real_spawn = dlsym(RTLD_NEXT, "posix_spawn");
    return real_spawn(pid, redirect_path(path), actions, attrp, argv, envp);
}

int posix_spawnp(pid_t *pid, const char *file,
                 const posix_spawn_file_actions_t *actions,
                 const posix_spawnattr_t *attrp,
                 char *const argv[], char *const envp[]) {
    static int (*real_spawnp)(pid_t *, const char *,
                              const posix_spawn_file_actions_t *,
                              const posix_spawnattr_t *,
                              char *const[], char *const[]) = NULL;
    if (!real_spawnp)
        real_spawnp = dlsym(RTLD_NEXT, "posix_spawnp");
    return real_spawnp(pid, redirect_path(file), actions, attrp, argv, envp);
}
