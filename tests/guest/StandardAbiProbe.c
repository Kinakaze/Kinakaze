/* Real Linux ELF: bounded NSS enumeration, SysV va_list and COPY data. */
typedef unsigned long size_t;
struct hostent { char *name; char **aliases; int family, length; char **addresses; };
struct protoent { char *name; char **aliases; int number; };
struct servent { char *name; char **aliases; int port; char *protocol; };
struct netent { char *name; char **aliases; int family; unsigned network; };
struct rpcent { char *name; char **aliases; int number; };
struct passwd { char *name, *password; unsigned uid, gid; char *gecos, *home, *shell; };
struct group { char *name, *password; unsigned gid; char **members; };
extern int strcmp(const char *, const char *);
extern int *__errno_location(void);
extern void _exit(int);
extern long write(int, const void *, size_t), read(int, void *, size_t);
extern int open(const char *, int, ...), close(int), fork(void), waitpid(int, int *, int);
extern char *setlocale(int, const char *);
extern int _nl_msg_cat_cntr;
extern void sethostent(int), endhostent(void), setprotoent(int), endprotoent(void);
extern void setservent(int), endservent(void), setnetent(int), endnetent(void);
extern void setrpcent(int), endrpcent(void), setpwent(void), endpwent(void), setgrent(void), endgrent(void);
extern struct hostent *gethostent(void);
extern struct protoent *getprotoent(void);
extern struct rpcent *getrpcent(void), *getrpcbyname(const char *), *getrpcbynumber(int);
extern int gethostent_r(struct hostent *, char *, size_t, struct hostent **, int *);
extern int getprotoent_r(struct protoent *, char *, size_t, struct protoent **);
extern int getservent_r(struct servent *, char *, size_t, struct servent **);
extern int getnetent_r(struct netent *, char *, size_t, struct netent **, int *);
extern int getnetbyname_r(const char *, struct netent *, char *, size_t, struct netent **, int *);
extern int getnetbyaddr_r(unsigned, int, struct netent *, char *, size_t, struct netent **, int *);
extern int getpwent_r(struct passwd *, char *, size_t, struct passwd **);
extern int getgrent_r(struct group *, char *, size_t, struct group **);
extern int __vdprintf_chk(int, int, const char *, __builtin_va_list);
extern int vdprintf(int, const char *, __builtin_va_list), vprintf(const char *, __builtin_va_list);
extern int msgget(int, int), msgsnd(int, const void *, size_t, int);
extern long msgrcv(int, void *, size_t, long, int);
extern int dprintf(int, const char *, ...);
extern int pthread_spin_init(unsigned *, int), pthread_spin_destroy(unsigned *);
extern int pthread_spin_lock(unsigned *), pthread_spin_trylock(unsigned *), pthread_spin_unlock(unsigned *);
extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *), pthread_join(unsigned long, void **);
extern void explicit_bzero(void *, size_t);
extern int sockatmark(int);
extern double remquo(double, double, int *);
extern float remquof(float, float, int *);
#define CHECK(x) do { if (!(x)) { dprintf(2, "standard ABI line %d: %s errno=%d\n", __LINE__, #x, *__errno_location()); return 1; } } while (0)

static int formatted(int fd, int checked, const char *fmt, ...) {
    __builtin_va_list args; __builtin_va_start(args, fmt);
    int result = checked == 2 ? vprintf(fmt, args) : checked ? __vdprintf_chk(fd, 1, fmt, args) : vdprintf(fd, fmt, args);
    __builtin_va_end(args); return result;
}

static struct { unsigned before, lock, after; } spin = {0x12345678, 0, 0x90abcdef};
static int increments;
static void *increment(void *unused) {
    (void)unused;
    for (int i = 0; i < 1000; i++) {
        pthread_spin_lock(&spin.lock); increments++; pthread_spin_unlock(&spin.lock);
    }
    return (void *)0;
}

static int run(void) {
    char bytes[513], tiny[1]; char *buf = bytes + 1; /* deliberately unaligned */
    int h_error = 0;
    int quotient = 0;
    CHECK(remquo(7.0, 2.0, &quotient) == -1.0 && (quotient & 7) == 4);
    CHECK(remquo(-7.0, 2.0, &quotient) == 1.0 && quotient < 0 && ((-quotient) & 7) == 4);
    CHECK(remquof(5.0f, 2.0f, &quotient) == 1.0f && (quotient & 7) == 2);
    CHECK(!pthread_spin_init(&spin.lock, 0) && spin.before == 0x12345678 && spin.after == 0x90abcdef);
    CHECK(!pthread_spin_lock(&spin.lock) && pthread_spin_trylock(&spin.lock) == 16);
    CHECK(!pthread_spin_unlock(&spin.lock));
    unsigned long threads[4];
    for (int i = 0; i < 4; i++) CHECK(!pthread_create(&threads[i], (void *)0, increment, (void *)0));
    for (int i = 0; i < 4; i++) CHECK(!pthread_join(threads[i], (void *)0));
    CHECK(increments == 4000 && !pthread_spin_destroy(&spin.lock));
    CHECK(spin.before == 0x12345678 && spin.after == 0x90abcdef);
    char secret[3] = {42, 42, 42}; explicit_bzero(secret + 1, 1);
    CHECK(secret[0] == 42 && secret[1] == 0 && secret[2] == 42);
    struct hostent host, *hp;
    sethostent(0);
    CHECK(gethostent_r(&host, tiny, 1, &hp, &h_error) == 34 && !hp);
    CHECK(!gethostent_r(&host, buf, 512, &hp, &h_error) && hp == &host && !h_error);
    CHECK(!strcmp(host.name, "localhost") && !strcmp(host.aliases[0], "loopback") && !host.aliases[1]);
    CHECK(host.family == 2 && host.length == 4 && (unsigned char)host.addresses[0][0] == 127 && !host.addresses[1]);
    CHECK(!gethostent_r(&host, buf, 512, &hp, &h_error) && host.family == 10 && host.length == 16);
    CHECK(gethostent_r(&host, buf, 512, &hp, &h_error) == 2 && !hp);
    sethostent(0); CHECK((hp = gethostent()) && !strcmp(hp->name, "localhost")); endhostent();
    struct protoent proto, *pp;
    setprotoent(0);
    CHECK(getprotoent_r(&proto, tiny, 1, &pp) == 34 && !pp);
    CHECK(!getprotoent_r(&proto, buf, 512, &pp) && pp == &proto && proto.number == 6);
    CHECK(!strcmp(proto.aliases[0], "TCP") && !proto.aliases[1]);
    int pid = fork(), status; CHECK(pid >= 0);
    if (!pid) { pp = getprotoent(); _exit(!pp || pp->number != 17); }
    CHECK(waitpid(pid, &status, 0) == pid && status == 0);
    CHECK(!getprotoent_r(&proto, buf, 512, &pp) && proto.number == 17);
    CHECK(getprotoent_r(&proto, buf, 512, &pp) == 2 && !pp); endprotoent();
    struct servent service, *sp;
    setservent(0); CHECK(getservent_r(&service, tiny, 1, &sp) == 34 && !sp);
    CHECK(!getservent_r(&service, buf, 512, &sp) && !strcmp(service.name, "http") && service.port == 20480);
    CHECK(!strcmp(service.aliases[0], "www") && !strcmp(service.protocol, "tcp"));
    CHECK(!getservent_r(&service, buf, 512, &sp) && !strcmp(service.name, "domain"));
    CHECK(getservent_r(&service, buf, 512, &sp) == 2 && !sp); endservent();
    struct netent net, *np;
    setnetent(0); CHECK(getnetent_r(&net, tiny, 1, &np, &h_error) == 34 && !np);
    CHECK(!getnetent_r(&net, buf, 512, &np, &h_error) && !strcmp(net.name, "loopback"));
    CHECK(!getnetent_r(&net, buf, 512, &np, &h_error) && !strcmp(net.name, "private"));
    CHECK(getnetent_r(&net, buf, 512, &np, &h_error) == 2 && !np); endnetent();
    CHECK(!getnetbyname_r("lan", &net, buf, 512, &np, &h_error) && np && net.network == 0x0a000000);
    CHECK(!getnetbyaddr_r(0x7f000000, 2, &net, buf, 512, &np, &h_error) && np && !strcmp(net.name, "loopback"));
    struct rpcent *rp = getrpcbynumber(100000); CHECK(rp && !strcmp(rp->name, "portmapper"));
    CHECK(!strcmp(rp->aliases[1], "sunrpc") && !rp->aliases[2]);
    rp = getrpcbyname("nfsprog"); CHECK(rp && rp->number == 100003);
    setrpcent(0); rp = getrpcent(); CHECK(rp && rp->number == 100000);
    rp = getrpcent(); CHECK(rp && rp->number == 100003); CHECK(!getrpcent()); endrpcent();
    struct passwd pw, *pwp;
    setpwent(); CHECK(getpwent_r(&pw, tiny, 1, &pwp) == 34 && !pwp);
    CHECK(!getpwent_r(&pw, buf, 512, &pwp) && pwp && pw.uid == 0 && !strcmp(pw.shell, "/bin/bash"));
    CHECK(!getpwent_r(&pw, buf, 512, &pwp) && pw.uid == 1000);
    CHECK(getpwent_r(&pw, buf, 512, &pwp) == 2 && !pwp); endpwent();
    struct group gr, *gp;
    setgrent(); CHECK(getgrent_r(&gr, tiny, 1, &gp) == 34 && !gp);
    CHECK(!getgrent_r(&gr, buf, 512, &gp) && gp && !strcmp(gr.members[0], "root") && !gr.members[1]);
    CHECK(!getgrent_r(&gr, buf, 512, &gp) && gr.gid == 1000);
    CHECK(getgrent_r(&gr, buf, 512, &gp) == 2 && !gp); endgrent();
    int counter = _nl_msg_cat_cntr;
    CHECK(setlocale(6, "C") && _nl_msg_cat_cntr != counter);
    counter = _nl_msg_cat_cntr;
    CHECK(setlocale(6, (void *)0) && _nl_msg_cat_cntr == counter);
    pid = fork(); CHECK(pid >= 0);
    if (!pid) { int before = _nl_msg_cat_cntr; _exit(before != counter || !setlocale(6, "C") || before == _nl_msg_cat_cntr); }
    CHECK(waitpid(pid, &status, 0) == pid && status == 0 && _nl_msg_cat_cntr == counter);
    int fd = open("/tmp/formatted", 0102|01000, 0600); CHECK(fd >= 0);
    CHECK(formatted(fd, 1, "%s %d %.2f %d%d%d%d%d%d%d\n", "abi", 42, 1.25, 1,2,3,4,5,6,7) == 20);
    CHECK(formatted(fd, 0, "%2$*1$d", 4, 7) == 4); CHECK(!close(fd));
    fd = open("/tmp/formatted", 0); CHECK(fd >= 0);
    CHECK(sockatmark(fd) == -1 && *__errno_location() == 88);
    long n = read(fd, buf, 511); CHECK(n > 0); buf[n] = 0;
    CHECK(!strcmp(buf, "abi 42 1.25 1234567\n   7")); CHECK(!close(fd));
    *__errno_location() = 0; CHECK(formatted(-1, 1, "%s", "bad") == -1 && *__errno_location() == 9);
    CHECK(formatted(1, 2, "%s", "vprintf OK\n") == 11);
    CHECK(sockatmark(-1) == -1 && *__errno_location() == 9);
    CHECK(msgget(1, 0) == -1 && *__errno_location() == 38);
    CHECK(msgsnd(1, buf, 1, 0) == -1 && *__errno_location() == 38);
    CHECK(msgrcv(1, buf, 1, 0, 0) == -1 && *__errno_location() == 38);
    return 0;
}
__attribute__((used)) static void entry(void) {
    int code = run(); if (!code) write(1, "STANDARD_ABI_OK\n", 16); _exit(code);
}
__attribute__((naked)) void _start(void) { __asm__ volatile("andq $-16, %rsp; call entry; ud2"); }
