/* GNU C cleanup ABI; compiled as a no-CRT Linux shared object by host clang.
 * Keep setjmp/longjmp within C, never across a Python callback frame. */
typedef unsigned long word;
typedef struct __attribute__((aligned(16))) {
    word registers[8]; int mask_saved; int pad; void *private[4];
} frame;
extern int __sigsetjmp(void *, int) __attribute__((returns_twice));
extern void __pthread_register_cancel(frame *);
extern void __pthread_unregister_cancel(frame *);
extern void __pthread_register_cancel_defer(frame *);
extern void __pthread_unregister_cancel_restore(frame *);
extern void __pthread_unwind_next(frame *) __attribute__((noreturn));
extern int pthread_create(word *, const void *, void *(*)(void *), void *);
extern int pthread_join(word, void **);
extern word pthread_self(void);
extern int pthread_cancel(word);
extern void pthread_testcancel(void);
extern void pthread_exit(void *) __attribute__((noreturn));
extern int pthread_key_create(unsigned *, void (*)(void *));
extern int pthread_key_delete(unsigned);
extern int pthread_setspecific(unsigned, const void *);
extern int pipe(int *);
extern int fork(void);
extern int close(int);
extern long read(int, void *, unsigned long);
extern long write(int, const void *, unsigned long);
extern int waitpid(int, int *, int);
extern int atexit(void (*)(void));
extern int sched_yield(void);
extern long syscall(long, ...);
static unsigned key;
static volatile unsigned used;
static volatile int trace[8];
static void record(int n) { trace[used++] = n; }
static void destroy(void *p) { record((int)(word)p); }
static void *worker(void *mode) {
    struct { frame value; volatile word guard[2]; } outer, inner;
    outer.guard[0] = inner.guard[0] = 0x123456789abcdefUL;
    outer.guard[1] = inner.guard[1] = 0xfedcba987654321UL;
    if (pthread_setspecific(key, (void *)9)) return (void *)101;
    if (__sigsetjmp(&outer.value, 0)) {
        record(outer.guard[0] == 0x123456789abcdefUL && outer.guard[1] == 0xfedcba987654321UL ? 1 : 101);
        __pthread_unwind_next(&outer.value);
    }
    __pthread_register_cancel(&outer.value);
    if (__sigsetjmp(&inner.value, 0)) {
        record(inner.guard[0] == 0x123456789abcdefUL && inner.guard[1] == 0xfedcba987654321UL ? 2 : 102);
        __pthread_unwind_next(&inner.value);
    }
    __pthread_register_cancel_defer(&inner.value);
    if ((word)mode == 1) pthread_exit((void *)0x1234);
    if ((word)mode == 2) { pthread_cancel(pthread_self()); pthread_testcancel(); }
    if ((word)mode == 3) pthread_cancel(pthread_self());
    __pthread_unregister_cancel_restore(&inner.value);
    __pthread_unregister_cancel(&outer.value);
    if ((word)mode == 3) { record(3); pthread_testcancel(); }
    return (void *)0x1234;
}
int probe_cleanup(int mode) {
    used = 0;
    if (pthread_key_create(&key, destroy)) return 1;
    word thread = 0;
    if (pthread_create(&thread, 0, worker, (void *)(word)mode)) return 2;
    void *result = 0;
    if (pthread_join(thread, &result)) return 3;
    pthread_key_delete(key);
    if (result != (void *)(mode >= 2 ? ~(word)0 : (word)0x1234)) return 4;
    if (!mode) return used == 1 && trace[0] == 9 ? 0 : 5;
    if (mode == 3) return used == 2 && trace[0] == 3 && trace[1] == 9 ? 0 : 7;
    return used == 3 && trace[0] == 2 && trace[1] == 1 && trace[2] == 9 ? 0 : 6;
}
static int completion_fd;
static unsigned main_done;
static void process_done(void) { write(completion_fd, "a", 1); }
static void *survivor(void *unused) {
    while (!__atomic_load_n(&main_done, __ATOMIC_ACQUIRE)) sched_yield();
    write(completion_fd, "y", 1);
    if (unused) syscall(60L, 0L); /* Mix a raw SYS_exit with pthread retirement. */
    return 0;
}
int probe_fork_cleanup(int with_survivor) {
    int fds[2];
    if (pipe(fds)) return 11;
    frame buffer;
    if (__sigsetjmp(&buffer, 0)) {
        char byte = 'x';
        write(fds[1], &byte, 1);
        __atomic_store_n(&main_done, 1, __ATOMIC_RELEASE);
        __pthread_unwind_next(&buffer);
    }
    __pthread_register_cancel(&buffer);
    int child = fork();
    if (!child) {
        close(fds[0]);
        completion_fd = fds[1];
        main_done = 0;
        if (atexit(process_done)) return 15;
        word thread;
        if (with_survivor && pthread_create(&thread, 0, survivor, (void *)(word)(with_survivor == 2))) return 16;
        pthread_exit(0);
    }
    __pthread_unregister_cancel(&buffer);
    close(fds[1]);
    if (child < 0) { close(fds[0]); return 12; }
    char bytes[4];
    long size = 0, n;
    while (size < 4 && (n = read(fds[0], bytes + size, 4 - size)) > 0) size += n;
    close(fds[0]);
    int status = -1;
    if (waitpid(child, &status, 0) != child || status) return 13;
    if (with_survivor == 2) return size >= 2 && bytes[0] == 'x' && bytes[1] == 'y' && (size == 2 || (size == 3 && bytes[2] == 'a')) ? 0 : 14;
    if (with_survivor) return size == 3 && bytes[0] == 'x' && bytes[1] == 'y' && bytes[2] == 'a' ? 0 : 14;
    return size == 2 && bytes[0] == 'x' && bytes[1] == 'a' ? 0 : 14;
}
