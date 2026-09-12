/* Freestanding Linux x86-64 process ABI integration probe.
 * Link against the generated libc.so.6 facade; no Windows runtime is linked.
 * argv[0] must resolve to this ELF in the configured guest filesystem.
 */
typedef unsigned long size_t;
typedef long ssize_t;
typedef int pid_t;
typedef struct { int allocated, used; void *actions; int pad[16]; } spawn_actions;
extern pid_t getpid(void), getppid(void), fork(void);
extern pid_t waitpid(pid_t, int *, int);
extern int pipe(int[2]), close(int), execve(const char *, char *const *, char *const *);
extern ssize_t read(int, void *, size_t), write(int, const void *, size_t);
extern int posix_spawn(pid_t *, const char *, const spawn_actions *, const void *, char *const *, char *const *);
extern int posix_spawn_file_actions_init(spawn_actions *);
extern int posix_spawn_file_actions_destroy(spawn_actions *);
extern int posix_spawn_file_actions_adddup2(spawn_actions *, int, int);
extern int posix_spawn_file_actions_addclose(spawn_actions *, int);
extern void _exit(int) __attribute__((noreturn));

static size_t length(const char *s) { size_t n = 0; while (s[n]) ++n; return n; }
static int equal(const char *a, const char *b) { while (*a && *a == *b) { ++a; ++b; } return *a == *b; }
static int decimal(const char *s) { int value = 0; while (*s >= '0' && *s <= '9') value = value * 10 + *s++ - '0'; return value; }
static void number(int value, char out[16]) {
    char reverse[16]; int n = 0, i = 0;
    do { reverse[n++] = '0' + value % 10; value /= 10; } while (value);
    while (n) out[i++] = reverse[--n]; out[i] = 0;
}
static void require(int condition, const char *message) {
    if (!condition) { write(2, message, length(message)); write(2, "\n", 1); _exit(90); }
}
static void wait_exact(pid_t child, int code) {
    int status = -1;
    require(waitpid(child, &status, 0) == child, "waitpid returned a different logical PID");
    require(status == (code << 8), "waitpid returned a different exit status");
}

__attribute__((noreturn)) void probe_start(size_t *stack) {
    int argc = (int)stack[0]; char **argv = (char **)(stack + 1);
    char *environment[] = { "PATH=/bin:/usr/bin", 0 };
    if (argc >= 3 && equal(argv[1], "--exec-child")) {
        require(getpid() == decimal(argv[2]), "exec changed the Linux PID");
        _exit(31);
    }
    if (argc >= 2 && equal(argv[1], "--spawn-child")) {
        require(write(1, "spawn-ok", 8) == 8, "spawn child write failed");
        _exit(7);
    }
    pid_t parent = getpid();
    require(parent > 0, "getpid failed");
    pid_t child = fork();
    require(child >= 0, "fork failed");
    if (!child) { require(getppid() == parent, "fork lost the logical parent PID"); _exit(23); }
    require(child != parent, "fork reused parent PID");
    wait_exact(child, 23);

    child = fork(); require(child >= 0, "exec test fork failed");
    if (!child) {
        char pid_text[16]; number(getpid(), pid_text);
        char *arguments[] = { argv[0], "--exec-child", pid_text, 0 };
        execve(argv[0], arguments, environment);
        require(0, "execve returned instead of replacing the image");
    }
    wait_exact(child, 31);

    int fds[2]; require(pipe(fds) == 0, "spawn pipe failed");
    spawn_actions actions;
    require(posix_spawn_file_actions_init(&actions) == 0, "spawn actions init failed");
    require(posix_spawn_file_actions_adddup2(&actions, fds[1], 1) == 0, "spawn dup action failed");
    require(posix_spawn_file_actions_addclose(&actions, fds[0]) == 0, "spawn close action failed");
    char *arguments[] = { argv[0], "--spawn-child", 0 };
    pid_t spawned = -999;
    require(posix_spawn(&spawned, argv[0], &actions, 0, arguments, environment) == 0, "posix_spawn failed");
    require(spawned > 0 && spawned != parent, "posix_spawn returned an invalid Linux PID");
    posix_spawn_file_actions_destroy(&actions); close(fds[1]);
    char output[9] = {0}; size_t used = 0;
    while (used < 8) { ssize_t n = read(fds[0], output + used, 8 - used); require(n > 0, "spawn output ended early"); used += (size_t)n; }
    require(equal(output, "spawn-ok"), "spawn dup2 did not redirect child stdout");
    close(fds[0]); wait_exact(spawned, 7);

    spawned = -999;
    require(posix_spawn(&spawned, "/__kinakaze_missing_executable__", 0, 0, arguments, environment) == 2,
            "missing spawn executable did not return ENOENT");
    require(spawned == -999, "failed spawn changed the caller's PID output");
    int status = -1;
    require(waitpid(-1, &status, 1) == -1, "failed spawn left an unreaped child");
    require(posix_spawn_file_actions_init(&actions) == 0, "second spawn actions init failed");
    require(posix_spawn_file_actions_adddup2(&actions, 99, 1) == 0, "invalid source action registration failed");
    require(posix_spawn(&spawned, argv[0], &actions, 0, arguments, environment) == 9,
            "invalid spawn dup2 did not return EBADF");
    posix_spawn_file_actions_destroy(&actions);
    write(1, "PROCESS_ABI_OK\n", 15);
    _exit(0);
}

__attribute__((naked, noreturn)) void _start(void) {
    __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
