typedef unsigned long size_t;
extern int prctl(int, ...), unshare(int), setns(int, int), fork(void);
extern int pipe(int *), close(int), open(const char *, int, ...);
extern int waitpid(int, int *, int), getpid(void), getppid(void);
extern int *__errno_location(void);
extern int snprintf(char *, size_t, const char *, ...), execve(const char *, char *const *, char *const *);
extern long read(int, void *, size_t), write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));

#define NEWPID 0x20000000
static void require(int condition, const char *why) {
    if (condition) return;
    size_t length = 0;
    while (why[length]) ++length;
    write(2, why, length);
    write(2, "\n", 1);
    _exit(93);
}

static void wait_status(int pid, int code, const char *why) {
    int status = -1;
    int result = waitpid(pid, &status, 0);
    if (result != pid || status != (code << 8)) {
        char detail[160];
        int length = snprintf(detail, sizeof(detail), "waitpid(%d)=%d status=%d errno=%d expected=%d\n",
                              pid, result, status, *__errno_location(), code << 8);
        write(2, detail, length);
        snprintf(detail, sizeof(detail), "/proc/%d/status", pid);
        int fd = open(detail, 0);
        if (fd >= 0) {
            char buffer[2048];
            long count = read(fd, buffer, sizeof(buffer));
            if (count > 0) write(2, buffer, count);
            close(fd);
        }
        require(0, why);
    }
}

__attribute__((used, noinline, noreturn)) static void probe_start(unsigned long *stack) {
    if (stack[0] == 2 && ((char *)stack[2])[0] == 'e') {
        require(getpid() > 1, "exec child must remain inside target PID namespace");
        require(getppid() == 0, "outer subreaper is not visible inside target namespace");
        int enabled = 0;
        require(!prctl(37, &enabled, 0ul, 0ul, 0ul) && enabled == 1,
                "exec must preserve the subreaper flag");
        _exit(37);
    }
    require(!prctl(36, 1ul, 0ul, 0ul, 0ul), "set child subreaper");
    int report[2], stop[2], release[2];
    require(!pipe(report) && !pipe(stop) && !pipe(release), "coordination pipes");
    int creator = fork();
    require(creator >= 0, "fork namespace creator");
    if (!creator) {
        require(!unshare(NEWPID), "unshare PID namespace for children");
        int init = fork();
        require(init >= 0, "fork namespace init");
        if (!init) {
            require(getpid() == 1, "namespace init PID");
            char byte;
            require(read(stop[0], &byte, 1) == 1, "keep namespace init alive");
            _exit(0);
        }
        require(write(report[1], &init, sizeof(init)) == sizeof(init), "report namespace init");
        _exit(0);
    }
    int init;
    require(read(report[0], &init, sizeof(init)) == sizeof(init), "read namespace init");
    wait_status(creator, 0, "collect namespace creator");
    char path[80];
    snprintf(path, sizeof(path), "/proc/%d/ns/pid", init);
    int ns = open(path, 0);
    require(ns >= 0, "open adopted init PID namespace");

    int launcher = fork();
    require(launcher >= 0, "fork exec launcher");
    if (!launcher) {
        require(!setns(ns, NEWPID), "enter namespace for future children");
        int child = fork();
        require(child >= 0, "fork child in joined namespace");
        if (!child) {
            char byte;
            require(read(release[0], &byte, 1) == 1, "wait for adoption before exec");
            int enabled = -1;
            require(!prctl(37, &enabled, 0ul, 0ul, 0ul) && enabled == 0,
                    "fork must not inherit the subreaper flag");
            require(!prctl(36, 1ul, 0ul, 0ul, 0ul), "set subreaper before exec");
            char *args[] = {"/probe", "exit", 0};
            char *env[] = {0};
            execve(args[0], args, env);
            require(0, "exec adopted namespace child");
        }
        require(write(report[1], &child, sizeof(child)) == sizeof(child), "report exec child");
        _exit(0);
    }
    int child;
    require(read(report[0], &child, sizeof(child)) == sizeof(child), "read exec child");
    wait_status(launcher, 0, "collect exec launcher");
    require(write(release[1], "x", 1) == 1, "release adopted child");
    wait_status(child, 37, "collect exact adopted exec status");
    require(write(stop[1], "x", 1) == 1, "stop namespace init");
    wait_status(init, 0, "collect adopted namespace init");
    close(ns);
    for (int i = 0; i < 2; ++i) { close(report[i]); close(stop[i]); close(release[i]); }
    write(1, "PIDNS_REAPER_EXEC_OK\n", 21);
    _exit(0);
}

__attribute__((naked, noreturn)) void _start(void) {
    __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
