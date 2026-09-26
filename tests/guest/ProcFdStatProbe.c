/* Real Linux ABI: live foreign anonymous inodes, identity and metadata changes. */
typedef unsigned long size_t;
struct stat {
    unsigned long dev, ino, nlink;
    unsigned mode, uid, gid, pad;
    unsigned long rdev;
    long size, blksize, blocks, atime, atime_ns, mtime, mtime_ns, ctime, ctime_ns, reserved[3];
};
_Static_assert(sizeof(struct stat) == 144, "Linux x86_64 stat");
extern int pipe(int *), socket(int, int, int), socketpair(int, int, int, int *);
extern int close(int), dup2(int, int), fork(void), waitpid(int, int *, int);
extern int stat(const char *, struct stat *), lstat(const char *, struct stat *), fstat(int, struct stat *);
extern int fchmod(int, unsigned), fchown(int, unsigned, unsigned);
extern int bind(int, const void *, unsigned), listen(int, int), connect(int, const void *, unsigned);
extern int getsockname(int, void *, unsigned *), accept(int, void *, unsigned *);
extern int memcmp(const void *, const void *, size_t), strcmp(const char *, const char *);
extern int snprintf(char *, size_t, const char *, ...), dprintf(int, const char *, ...), *__errno_location(void);
extern long read(int, void *, size_t), write(int, const void *, size_t), readlink(const char *, char *, size_t);
extern void _exit(int);
#define CHECK(x) do { if (!(x)) { dprintf(2, "proc fd stat line %d: %s errno=%d\n", __LINE__, #x, *__errno_location()); return 1; } } while (0)
struct item { int fd, pad; struct stat value; };
struct packet { int count, stage; struct item items[16]; };
static int transfer(int fd, void *data, size_t bytes, int output) {
    char *p = data;
    while (bytes) {
        long n = output ? write(fd, p, bytes) : read(fd, p, bytes);
        if (n <= 0) return 0;
        p += n; bytes -= n;
    }
    return 1;
}
static int child(int out, int ack, int inherited) {
    int p[2], u[2]; CHECK(!pipe(p) && !socketpair(1, 1, 0, u));
    int fds[] = { p[0], p[1], u[0], u[1], socket(2, 1, 0), socket(2, 2, 0),
                  socket(10, 2, 0), socket(16, 3, 0), dup2(p[0], 210), dup2(u[0], 211), inherited };
    int count = sizeof(fds) / sizeof(*fds);
    for (int i = 0; i < count; ++i) CHECK(fds[i] >= 0);
    struct { unsigned short family, port; unsigned address; char zero[8]; } address = {2, 0, 0x0100007f, {0}};
    unsigned length = sizeof(address);
    CHECK(!bind(fds[4], &address, sizeof(address)) && !listen(fds[4], 2));
    CHECK(!getsockname(fds[4], &address, &length));
    int client = socket(2, 1, 0); CHECK(client >= 0 && !connect(client, &address, length));
    int accepted = accept(fds[4], 0, 0); CHECK(accepted >= 0);
    struct packet packet = {0}; packet.count = count + 1;
    for (int stage = 0; stage != 3; ++stage) {
        if (stage == 1) {
            for (int i = 0; i < count; ++i) CHECK(!fchmod(fds[i], 0640) && !fchown(fds[i], 1234, 2345));
            CHECK(!fchmod(accepted, 0620) && !fchown(accepted, 1234, 2345));
            char byte = 'x'; CHECK(write(p[1], &byte, 1) == 1 && read(p[0], &byte, 1) == 1);
        }
        if (stage == 2) CHECK(dup2(u[1], 210) == 210);
        packet.stage = stage;
        for (int i = 0; i < count; ++i) {
            packet.items[i].fd = fds[i]; CHECK(!fstat(fds[i], &packet.items[i].value));
        }
        packet.items[count].fd = accepted; CHECK(!fstat(accepted, &packet.items[count].value));
        CHECK(transfer(out, &packet, sizeof(packet), 1));
        char byte; CHECK(read(ack, &byte, 1) == 1);
    }
    for (int i = 0; i < count; ++i) CHECK(!close(fds[i]));
    CHECK(!close(accepted) && !close(client));
    packet.stage = 3; CHECK(transfer(out, &packet, sizeof(packet), 1));
    char byte; CHECK(read(ack, &byte, 1) == 1);
    return 0;
}
static int run(void) {
    int ready[2], ack[2]; CHECK(!pipe(ready) && !pipe(ack));
    int inherited = socket(2, 2, 0); CHECK(inherited >= 0);
    struct stat initial; CHECK(!fstat(inherited, &initial));
    int pid = fork(); CHECK(pid >= 0);
    if (!pid) { close(ready[0]); close(ack[1]); _exit(child(ready[1], ack[0], inherited)); }
    close(ready[1]); close(ack[0]);
    struct packet packet;
    for (int stage = 0; stage != 4; ++stage) {
        CHECK(transfer(ready[0], &packet, sizeof(packet), 0) && packet.stage == stage);
        for (int i = 0; i < packet.count; ++i) {
            struct stat actual, link; char path[80], target[80], expected[80];
            struct item *item = &packet.items[i];
            snprintf(path, sizeof(path), "/proc/%d/fd/%d", pid, item->fd);
            if (stage == 3) { CHECK(stat(path, &actual) == -1 && *__errno_location() == 2); continue; }
            CHECK(!stat(path, &actual));
            if (memcmp(&actual, &item->value, sizeof(actual))) {
                dprintf(2, "foreign stat differs stage=%d fd=%d ino=%lu/%lu uid=%u/%u\n", stage, item->fd, actual.ino, item->value.ino, actual.uid, item->value.uid);
                return 1;
            }
            CHECK(actual.ino && actual.nlink == 1 && !actual.rdev && !actual.size && !actual.blocks);
            CHECK(actual.blksize == 4096 && actual.ctime > 1700000000 && actual.ctime_ns < 1000000000);
            CHECK(!lstat(path, &link) && (link.mode & 0170000) == 0120000);
            long n = readlink(path, target, sizeof(target) - 1); CHECK(n > 0); target[n] = 0;
            snprintf(expected, sizeof(expected), "%s:[%lu]", (actual.mode & 0170000) == 0010000 ? "pipe" : "socket", actual.ino);
            CHECK(!strcmp(target, expected));
            if (stage && stage != 3) CHECK(actual.uid == 1234 && actual.gid == 2345);
        }
        if (!stage) CHECK(packet.items[10].value.ino == initial.ino);
        CHECK(write(ack[1], "!", 1) == 1);
    }
    int status; CHECK(waitpid(pid, &status, 0) == pid && !status);
    CHECK(!fstat(inherited, &initial) && initial.uid == 1234 && initial.gid == 2345);
    close(inherited); close(ready[0]); close(ack[1]);
    dprintf(1, "PROC_FD_STAT_OK\n"); return 0;
}
__attribute__((used)) static void entry(void) { _exit(run()); }
__attribute__((naked)) void _start(void) { __asm__ volatile("andq $-16, %rsp; call entry; ud2"); }
