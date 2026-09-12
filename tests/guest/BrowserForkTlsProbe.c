// Chromium's zygote forks before creating V8 background compiler threads.
// The surviving thread keeps its TLS values; new threads need the ELF template.
extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *);
extern int pthread_join(unsigned long, void **);
extern int fork(void);
extern int waitpid(int, int *, int);
extern long write(int, const void *, unsigned long);
extern void _exit(int) __attribute__((noreturn));

static __thread volatile unsigned long initialized = 0x1234abcd;
static __thread volatile unsigned long uninitialized[17];

static void *fresh_thread(void *unused) {
    (void)unused;
    if (initialized != 0x1234abcd) return (void *)1;
    for (unsigned i = 0; i < 17; ++i) if (uninitialized[i]) return (void *)2;
    initialized = 77;
    uninitialized[16] = 88;
    return 0;
}

static int check_thread(void) {
    unsigned long thread;
    void *result = (void *)3;
    if (pthread_create(&thread, 0, fresh_thread, 0)) return 3;
    if (pthread_join(thread, &result)) return 4;
    return (int)(unsigned long)result;
}

static int check_fork(unsigned depth) {
    int child = fork(), status = -1;
    if (child < 0) return 5;
    if (!child) {
        if (initialized != 55 || uninitialized[16] != 66) _exit(6);
        int result = check_thread();
        if (!result && depth) result = check_fork(depth - 1);
        if (initialized != 55 || uninitialized[16] != 66) result = 7;
        _exit(result);
    }
    if (waitpid(child, &status, 0) != child) return 8;
    return status ? 9 : 0;
}

__attribute__((noreturn)) void browser_tls_main(void) {
    int result = check_thread();
    if (!result && (initialized != 0x1234abcd || uninitialized[16])) result = 10;
    initialized = 55;
    uninitialized[16] = 66;
    if (!result) result = check_fork(1);
    if (!result) write(1, "BROWSER_FORK_TLS_OK\n", 20);
    _exit(result);
}

__asm__(".text\n.global _start\n.type _start,@function\n_start:\n"
        "xor %ebp,%ebp\nand $-16,%rsp\ncall browser_tls_main\nud2\n");
