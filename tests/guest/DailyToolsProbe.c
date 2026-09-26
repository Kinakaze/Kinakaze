/* Linux ELF integration: getent databases, netlink buffers and proc magic links. */
typedef unsigned long size_t;
struct aliasent { char *name; size_t count; char **members; int local; };
struct sgrp { char *name, *password; char **admins, **members; };
struct dynarray { size_t used, allocated; void *array; };
struct hostent { char *name; char **aliases; int family, length; char **addresses; };
extern struct hostent *gethostbyname(const char *), *gethostbyaddr(const void *, unsigned, int), *gethostent(void);
extern void sethostent(int), endhostent(void);
extern int strcmp(const char *, const char *), snprintf(char *, size_t, const char *, ...);
extern int *__errno_location(void), dprintf(int, const char *, ...);
extern void _exit(int), free(void *);
extern int fork(void), waitpid(int, int *, int), pipe(int *), close(int), open(const char *, int, ...);
extern int chdir(const char *), chroot(const char *), mkdir(const char *, unsigned), getpid(void);
extern long write(int, const void *, size_t), read(int, void *, size_t), readlink(const char *, char *, size_t);
extern long syscall(long, ...);
extern void setaliasent(void), endaliasent(void), setsgent(void), endsgent(void);
extern struct aliasent *getaliasent(void), *getaliasbyname(const char *);
extern struct sgrp *getsgent(void), *getsgnam(const char *);
extern int putsgent(const struct sgrp *, void *);
extern void *fopen(const char *, const char *);
extern int fclose(void *);
extern int ether_hostton(const char *, unsigned char *), ether_ntohost(char *, const unsigned char *);
extern int __nss_configure_lookup(const char *, const char *);
extern _Bool __libc_dynarray_resize(struct dynarray *, size_t, void *, size_t);
extern int socket(int, int, int), setsockopt(int, int, int, const void *, unsigned);
extern int getsockopt(int, int, int, void *, unsigned *);
extern long sendto(int, const void *, size_t, int, const void *, unsigned);
extern long send(int, const void *, size_t, int);
struct iovec { void *base; size_t length; };
struct msghdr { void *name; unsigned name_length; struct iovec *vectors; size_t count; void *control; size_t control_length; int flags; };
extern long recvmsg(int, struct msghdr *, int), recv(int, void *, size_t, int);
#define CHECK(x) do { if (!(x)) { dprintf(2, "daily tools line %d: %s errno=%d\n", __LINE__, #x, *__errno_location()); return 1; } } while (0)
static int equal_link(const char *path, const char *want) {
    char value[4096]; long n = readlink(path, value, sizeof(value)-1);
    if (n < 0) return 0; value[n] = 0;
    if (strcmp(value, want)) dprintf(2, "link %s = %s, expected %s\n", path, value, want);
    return !strcmp(value, want);
}
static int run(void) {
    dprintf(2, "CHECK aliases\n");
    setaliasent(); struct aliasent *a = getaliasent();
    CHECK(a && !strcmp(a->name, "team") && a->count == 3 && !strcmp(a->members[2], "carol") && !a->members[3]);
    CHECK(getaliasent() && !getaliasent()); endaliasent();
    a = getaliasbyname("TEAM"); CHECK(a && a->count == 3); CHECK(!getaliasbyname("missing"));
    dprintf(2, "CHECK gshadow\n");
    setsgent(); struct sgrp *g = getsgent();
    CHECK(g && !strcmp(g->name, "team") && !strcmp(g->password, "!") && !strcmp(g->admins[0], "alice") && !g->admins[1]);
    CHECK(!strcmp(g->members[1], "carol") && !g->members[2]);
    void *f = fopen("/tmp/gshadow-output", "w"); CHECK(f && !putsgent(g, f) && !fclose(f));
    char data[128]; int fd = open("/tmp/gshadow-output", 0); CHECK(fd >= 0);
    long n = read(fd, data, sizeof(data)-1); CHECK(n > 0); data[n] = 0; close(fd);
    CHECK(!strcmp(data, "team:!:alice:bob,carol\n"));
    CHECK(getsgent() && !getsgent()); endsgent();
    g = getsgnam("empty"); CHECK(g && !*g->admins && !*g->members); CHECK(!getsgnam("missing"));
    dprintf(2, "CHECK ethers\n");
    unsigned char mac[6]; CHECK(!ether_hostton("printer", mac) && mac[0] == 2 && mac[5] == 255);
    CHECK(!ether_ntohost(data, mac) && !strcmp(data, "printer"));
    CHECK(ether_hostton("missing", mac) == -1);
    dprintf(2, "CHECK nss\n");
    CHECK(!__nss_configure_lookup("hosts", "files"));
    struct hostent *host = gethostbyname("alias-host");
    CHECK(host && !strcmp(host->name, "fixture-host") && host->family == 2);
    unsigned char loopback[4] = {127, 0, 0, 1};
    host = gethostbyaddr(loopback, 4, 2); CHECK(host && !strcmp(host->name, "fixture-host"));
    CHECK(!__nss_configure_lookup("hosts", "dns") && !gethostent());
    CHECK(!__nss_configure_lookup("hosts", "files")); sethostent(0);
    CHECK(gethostent() && !gethostent()); endhostent();
    CHECK(__nss_configure_lookup("passwd", "unimplemented") == -1 && *__errno_location() == 95);
    dprintf(2, "CHECK dynarray\n");
    unsigned scratch[2] = {41, 42}; struct dynarray array = {2, 2, scratch};
    CHECK(__libc_dynarray_resize(&array, 5, scratch, sizeof(unsigned)));
    CHECK(array.used == 5 && array.allocated >= 5 && ((unsigned *)array.array)[1] == 42);
    CHECK(__libc_dynarray_resize(&array, 32, scratch, sizeof(unsigned)) && ((unsigned *)array.array)[0] == 41);
    CHECK(!__libc_dynarray_resize(&array, (size_t)-1, scratch, sizeof(unsigned)) && *__errno_location() == 12);
    CHECK(array.used == 32); free(array.array);
    dprintf(2, "CHECK netlink\n");
    int sock = socket(16, 3, 0); CHECK(sock >= 0); int value = 32768; unsigned length = 4;
    CHECK(!setsockopt(sock, 1, 7, &value, 4) && !setsockopt(sock, 1, 8, &value, 4));
    value = 0; CHECK(!getsockopt(sock, 1, 7, &value, &length) && value == 65536 && length == 4);
    CHECK(!getsockopt(sock, 1, 8, &value, &length) && value == 65536);
    CHECK(setsockopt(sock, 1, 7, &value, 1) == -1 && *__errno_location() == 22);
    struct { unsigned length; unsigned short type, flags; unsigned sequence, pid; } noop = {16, 1, 1, 1, 0};
    CHECK(send(sock, &noop, sizeof(noop), 0) == sizeof(noop));
    struct { unsigned length; unsigned short type, flags; unsigned sequence, pid; char link[16]; } dump = {32, 18, 0x301, 2, 0, {0}};
    CHECK(send(sock, &dump, sizeof(dump), 0) == sizeof(dump));
    struct msghdr message = {0};
    long full = recvmsg(sock, &message, 0x22); CHECK(full >= 16 && (message.flags & 0x20));
    struct iovec vector = {data, 1}; message.vectors = &vector; message.count = 1;
    CHECK(recvmsg(sock, &message, 2) == 1 && (message.flags & 0x20));
    CHECK(recv(sock, data, 1, 0x20) == full);
    for (;;) {
        unsigned packet[8192]; long count = recv(sock, packet, sizeof(packet), 0x40);
        CHECK(count >= 16);
        if ((packet[1] & 0xffff) == 3) { CHECK(count >= 20 && packet[4] == 0); break; }
    }
    struct { unsigned length; unsigned short type, flags; unsigned sequence, pid; char address[8], reserved[128]; } addrs = {24, 22, 0x301, 3, 0, {0}, {0}};
    CHECK(send(sock, &addrs, sizeof(addrs), 0) == sizeof(addrs));
    int address_count = 0;
    for (;;) {
        unsigned packet[8192]; long count = recv(sock, packet, sizeof(packet), 0x40);
        CHECK(count >= 16);
        if ((packet[1] & 0xffff) == 3) { CHECK(count >= 20 && packet[4] == 0); break; }
        CHECK((packet[1] & 0xffff) == 20); address_count++;
    }
    CHECK(address_count > 0);
    struct { unsigned short family, pad; unsigned pid, groups; } nl = {16, 0, 0, 0};
    static char large[65536]; CHECK(sendto(sock, large, sizeof(large), 0, &nl, sizeof(nl)) == -1 && *__errno_location() == 90);
    int child = fork(), status = -1; CHECK(child >= 0);
    if (!child) { value = 0; length = 4; _exit(getsockopt(sock, 1, 7, &value, &length) || value != 65536); }
    CHECK(waitpid(child, &status, 0) == child && status == 0); close(sock);
    dprintf(2, "CHECK proc\n");
    mkdir("/tmp/jail", 0755); mkdir("/tmp/jail/work", 0755);
    fd = open("/tmp/jail/work/token", 65, 0644); CHECK(fd >= 0 && write(fd, "ok", 2) == 2); close(fd);
    CHECK(!chdir("/tmp") && equal_link("/proc/self/cwd", "/tmp") && equal_link("/proc/self/root", "/"));
    struct { unsigned long flags, mode, resolve; } how = {0, 0, 2};
    CHECK(syscall(437, -100, "/proc/self/cwd", &how, sizeof(how)) == -1 && *__errno_location() == 40);
    int ready[2], done[2]; CHECK(!pipe(ready) && !pipe(done));
    child = fork(); CHECK(child >= 0);
    if (!child) {
        close(ready[0]); close(done[1]);
        if (chroot("/tmp/jail") || chdir("/work")) _exit(2);
        if (write(ready[1], "!", 1) != 1 || read(done[0], data, 1) != 1) _exit(3);
        _exit(0);
    }
    close(ready[1]); close(done[0]); CHECK(read(ready[0], data, 1) == 1);
    char path[128]; snprintf(path, sizeof(path), "/proc/%d/cwd", child);
    CHECK(equal_link(path, "/tmp/jail/work"));
    snprintf(path, sizeof(path), "/proc/%d/root", child); CHECK(equal_link(path, "/tmp/jail"));
    snprintf(path, sizeof(path), "/proc/%d/cwd/token", child); fd = open(path, 0);
    CHECK(fd >= 0 && read(fd, data, 2) == 2 && data[0] == 'o' && data[1] == 'k'); close(fd);
    CHECK(write(done[1], "!", 1) == 1 && waitpid(child, &status, 0) == child && !status);
    CHECK(equal_link("/proc/self/exe", "/probe"));
    dprintf(1, "DAILY_TOOLS_OK\n"); return 0;
}
__attribute__((used)) static void entry(void) { _exit(run()); }
__attribute__((naked)) void _start(void) { __asm__ volatile("andq $-16, %rsp; call entry; ud2"); }
