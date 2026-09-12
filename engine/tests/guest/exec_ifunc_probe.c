/* An actual GNU IFUNC relocation must execute this resolver before _start. */
typedef unsigned long size_t;
typedef unsigned long long u64;
typedef unsigned int u32;
struct timespec { long seconds, nanoseconds; };
struct state {
    u64 counter;
    u32 phase, seen, thread_ready, parent_pid, stop;
    u32 blocked_ready, blocked_late_return;
};
extern int open(const char *, int, ...), close(int), getpid(void);
extern void *mmap(void *, size_t, int, int, int, long);
extern int nanosleep(const struct timespec *, struct timespec *);
extern long write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));

static struct state *resolved_state;
static int resolver_ran;
static int gate_passed(void) { return 73; }
static int gate_failed(void) { return -1; }

static int (*resolve_exec_gate(void))(void) {
    resolver_ran = 1;
    int fd = open("/exec-thread-state.bin", 2);
    if (fd < 0) return gate_failed;
    struct state *shared = mmap(0, 4096, 1 | 2, 1, fd, 0);
    close(fd);
    if (shared == (void *)-1) return gate_failed;
    resolved_state = shared;
    u64 before = __atomic_load_n(&shared->counter, __ATOMIC_SEQ_CST);
    __atomic_store_n(&shared->phase, 1, __ATOMIC_SEQ_CST);
    struct timespec duration = {0, 100000000};
    if (nanosleep(&duration, 0) != 0) return gate_failed;
    return __atomic_load_n(&shared->counter, __ATOMIC_SEQ_CST) == before
        && __atomic_load_n(&shared->seen, __ATOMIC_SEQ_CST) == 0
        && __atomic_load_n(&shared->thread_ready, __ATOMIC_SEQ_CST) == 3
        && __atomic_load_n(&shared->blocked_ready, __ATOMIC_SEQ_CST) == 1
        && __atomic_load_n(&shared->blocked_late_return, __ATOMIC_SEQ_CST) == 0
        ? gate_passed : gate_failed;
}

int exec_gate(void) __attribute__((ifunc("resolve_exec_gate")));

__attribute__((noreturn)) void probe_start(size_t *initial_stack) {
    (void)initial_stack;
    if (exec_gate() != 73 || resolver_ran != 1 || !resolved_state) {
        const char error[] = "thread exec probe: IFUNC observed surviving sibling activity\n";
        write(2, error, sizeof(error) - 1);
        _exit(92);
    }
    if ((u32)getpid() != resolved_state->parent_pid) {
        const char error[] = "thread exec probe: exec changed logical PID\n";
        write(2, error, sizeof(error) - 1);
        _exit(93);
    }
    const char completed[] = "THREAD_EXEC_IFUNC_OK\n";
    write(1, completed, sizeof(completed) - 1);
    _exit(0);
}

__attribute__((naked, noreturn)) void _start(void) {
    __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
