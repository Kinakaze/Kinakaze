set -eu
work=$(/bin/busybox mktemp -d /tmp/kinakaze-exit.XXXXXX)
trap 'cd /; /bin/busybox rm -rf "$work"' EXIT
cd "$work"
/bin/busybox cat > base.c <<'C'
#include <stdlib.h>
extern void emit(const char *);
extern void *__dso_handle;
extern int __cxa_atexit(void (*)(void *), void *, void *);
static int alive = 1;
int base_ping(void) { return alive; }
static void cxa(void *p) { if (p != &alive) _Exit(91); emit("base-cxa"); }
__attribute__((constructor)) static void init(void) {
    if (__cxa_atexit(cxa, &alive, __dso_handle)) _Exit(92);
}
__attribute__((destructor)) static void fini(void) { emit("base-fini"); alive = 0; }
C
/bin/busybox cat > middle.c <<'C'
#include <stdlib.h>
extern void emit(const char *);
extern int base_ping(void);
extern void *__dso_handle;
extern int __cxa_atexit(void (*)(void *), void *, void *);
extern void __cxa_finalize(void *);
int middle_ping(void) { return base_ping(); }
static void cxa(void *p) { if (!base_ping() || p) _Exit(93); emit("middle-cxa"); }
__attribute__((constructor)) static void init(void) {
    if (__cxa_atexit(cxa, 0, __dso_handle)) _Exit(94);
}
__attribute__((destructor)) static void fini(void) {
    if (!base_ping()) _Exit(95);
    emit("middle-fini");
    __cxa_finalize(__dso_handle);
    __cxa_finalize(__dso_handle);
}
C
/bin/busybox cat > main.c <<'C'
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
extern int middle_ping(void);
extern int __cxa_atexit(void (*)(void *), void *, void *);
extern void __cxa_finalize(void *);
static int fd = -1, mode, selected_owner;
void emit(const char *s) {
    size_t n = strlen(s);
    if (fd < 0 || write(fd, s, n) != (ssize_t)n || write(fd, "\n", 1) != 1) _Exit(90);
}
static void child_log(void) {
    close(fd);
    fd = open("child.log", O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) _Exit(89);
}
static void fork_callback(const char *marker) {
    pid_t child = fork();
    if (child < 0) _Exit(88);
    if (!child) { child_log(); emit(marker); return; }
    int status;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status)) _Exit(87);
}
static void ctor_exit(void) { emit("main-ctor"); }
__attribute__((constructor)) static void ctor(void) { if (atexit(ctor_exit)) _Exit(86); }
static void early(void) { emit("early"); }
static void added(void) { emit("added"); }
static void selected(void *p) {
    if (p != &selected_owner) _Exit(85);
    emit("selected");
    __cxa_finalize(&selected_owner);
}
static void late(void) {
    emit("late");
    if (mode == 3) fork_callback("handler-child");
    if (atexit(added)) _Exit(84);
    if (mode == 5) exit(31);
}
__attribute__((destructor(201))) static void main_low(void) { emit("main-low"); }
__attribute__((destructor(202))) static void main_high(void) {
    emit("main-high");
    void *h = dlopen("libbase.so", RTLD_NOW | RTLD_NOLOAD);
    if (!h || dlclose(h)) _Exit(83);
    if (mode == 4) fork_callback("fini-child");
}
int main(int argc, char **argv) {
    if (argc != 2 || !middle_ping()) return 82;
    mode = atoi(argv[1]);
    fd = open("parent.log", O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || atexit(early) || __cxa_atexit(selected, &selected_owner, &selected_owner) || atexit(late)) return 81;
    if (mode == 1) _exit(0);
    if (mode == 2) {
        pid_t child = fork();
        if (child < 0) return 80;
        if (!child) { child_log(); exit(23); }
        int status;
        if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 23) return 79;
    }
    return 0;
}
C
/usr/bin/gcc -shared -fPIC base.c -Wl,-soname,libbase.so -o libbase.so
/usr/bin/gcc -shared -fPIC middle.c -L. -lbase -Wl,-rpath,'$ORIGIN' -o libmiddle.so
/usr/bin/gcc main.c -rdynamic -L. -lmiddle -Wl,-rpath,'$ORIGIN' -ldl -o main
/bin/busybox cat > expected.log <<'LOG'
late
added
selected
early
main-ctor
main-high
main-low
middle-fini
middle-cxa
base-fini
base-cxa
LOG
./main 0
/bin/busybox cmp parent.log expected.log
./main 1
test ! -s parent.log
./main 2
/bin/busybox cmp parent.log expected.log
/bin/busybox cmp child.log expected.log
./main 3
/bin/busybox cmp parent.log expected.log
{ printf 'handler-child\n'; /bin/busybox tail -n +2 expected.log; } > expected-child.log
/bin/busybox cmp child.log expected-child.log
./main 4
/bin/busybox cmp parent.log expected.log
{ printf 'fini-child\n'; /bin/busybox tail -n +7 expected.log; } > expected-child.log
/bin/busybox cmp child.log expected-child.log
status=0
./main 5 || status=$?
test "$status" -eq 31
/bin/busybox cmp parent.log expected.log
printf 'ELF_EXIT_ORDER_REENTRY_FORK_DESTRUCTORS_OK\n'
