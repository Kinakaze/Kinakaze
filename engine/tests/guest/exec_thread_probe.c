/* Linux x86-64: failed exec preserves sibling threads; successful exec retires
 * every sibling before the replacement's IFUNC resolver can run. */
typedef unsigned long size_t;
typedef unsigned long pthread_t;
typedef unsigned long long u64;
typedef unsigned int u32;
struct timespec { long seconds, nanoseconds; };
struct state {
    u64 counter;
    u32 phase, seen, thread_ready, parent_pid, stop;
    u32 blocked_ready, blocked_late_return;
};
struct blocked_arguments { struct state *shared; int fd; };
extern int open(const char *, int, ...), close(int), getpid(void);
extern int ftruncate(int, long), execve(const char *, char *const *, char *const *);
extern void *mmap(void *, size_t, int, int, int, long);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int nanosleep(const struct timespec *, struct timespec *);
extern long write(int, const void *, size_t);
extern int pipe(int[2]);
extern long read(int, void *, size_t);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));

static size_t length(const char *text) { size_t n = 0; while (text[n]) ++n; return n; }
static void require(int value, const char *error) {
    if (!value) { write(2, error, length(error)); write(2, "\n", 1); _exit(91); }
}
static void delay(void) {
    struct timespec duration = {0, 1000000};
    nanosleep(&duration, 0);
}
static void *busy_sibling(void *argument) {
    struct state *shared = argument;
    __atomic_fetch_add(&shared->thread_ready, 1, __ATOMIC_SEQ_CST);
    while (!__atomic_load_n(&shared->stop, __ATOMIC_SEQ_CST)) {
        if (__atomic_load_n(&shared->phase, __ATOMIC_SEQ_CST))
            __atomic_store_n(&shared->seen, 1, __ATOMIC_SEQ_CST);
        __atomic_fetch_add(&shared->counter, 1, __ATOMIC_SEQ_CST);
        __asm__ volatile("pause");
    }
    return 0;
}
static void *blocked_sibling(void *argument) {
    struct blocked_arguments *input = argument;
    __atomic_store_n(&input->shared->blocked_ready, 1, __ATOMIC_SEQ_CST);
    char byte;
    read(input->fd, &byte, 1);
    if (__atomic_load_n(&input->shared->phase, __ATOMIC_SEQ_CST))
        __atomic_store_n(&input->shared->blocked_late_return, 1, __ATOMIC_SEQ_CST);
    return 0;
}

__attribute__((noreturn)) void probe_start(size_t *initial_stack) {
    (void)initial_stack;
    int fd = open("/exec-thread-state.bin", 2 | 64 | 512, 0600);
    require(fd >= 0, "thread exec probe: shared state open failed");
    require(ftruncate(fd, 4096) == 0, "thread exec probe: shared state resize failed");
    struct state *shared = mmap(0, 4096, 1 | 2, 1, fd, 0);
    require(shared != (void *)-1, "thread exec probe: shared state mmap failed");
    close(fd);
    shared->counter = 0;
    shared->phase = shared->seen = shared->thread_ready = shared->stop = 0;
    shared->blocked_ready = shared->blocked_late_return = 0;
    shared->parent_pid = (u32)getpid();
    require(shared->parent_pid > 0, "thread exec probe: logical PID unavailable");
    pthread_t threads[3];
    for (int i = 0; i < 3; ++i)
        require(pthread_create(&threads[i], 0, busy_sibling, shared) == 0,
                "thread exec probe: pthread_create failed");
    int waits = 0;
    while (__atomic_load_n(&shared->thread_ready, __ATOMIC_SEQ_CST) != 3 && waits++ < 1000)
        delay();
    require(__atomic_load_n(&shared->thread_ready, __ATOMIC_SEQ_CST) == 3,
            "thread exec probe: sibling threads did not start");
    int blocked_pipe[2];
    require(pipe(blocked_pipe) == 0, "thread exec probe: blocking pipe creation failed");
    struct blocked_arguments blocked = {shared, blocked_pipe[0]};
    pthread_t reader;
    require(pthread_create(&reader, 0, blocked_sibling, &blocked) == 0,
            "thread exec probe: blocked reader creation failed");
    waits = 0;
    while (!__atomic_load_n(&shared->blocked_ready, __ATOMIC_SEQ_CST) && waits++ < 1000)
        delay();
    require(__atomic_load_n(&shared->blocked_ready, __ATOMIC_SEQ_CST),
            "thread exec probe: blocked reader did not start");
    for (int i = 0; i < 10; ++i) delay();

    char *environment[] = {"PATH=/bin:/usr/bin", 0};
    char *missing[] = {"/__missing_thread_exec_probe__", 0};
    u64 before = __atomic_load_n(&shared->counter, __ATOMIC_SEQ_CST);
    require(execve(missing[0], missing, environment) == -1 && *__errno_location() == 2,
            "thread exec probe: missing exec did not return ENOENT");
    waits = 0;
    while (__atomic_load_n(&shared->counter, __ATOMIC_SEQ_CST) == before && waits++ < 1000)
        delay();
    require(__atomic_load_n(&shared->counter, __ATOMIC_SEQ_CST) != before,
            "thread exec probe: failed exec stopped sibling threads");
    const char recovered[] = "THREAD_FAILED_EXEC_RECOVERED\n";
    write(1, recovered, sizeof(recovered) - 1);

    char *target[] = {"/exec_ifunc_probe", 0};
    execve(target[0], target, environment);
    require(0, "thread exec probe: replacement exec returned");
    _exit(91);
}

__attribute__((naked, noreturn)) void _start(void) {
    __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
