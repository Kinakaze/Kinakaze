/* Explicit library handles must reach the same caller-aware loader ABI. */
typedef unsigned long size_t;
extern void *dlopen(const char *, int), *dlsym(void *, const char *);
extern void *dlvsym(void *, const char *, const char *);
extern const char *dlerror(void);
extern int dlclose(void *), fork(void), waitpid(int, int *, int);
extern int dlinfo(void *, int, void *);
extern long write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));
typedef void *(*lookup_fn)(void *, const char *);
typedef void *(*version_lookup_fn)(void *, const char *, const char *);
struct LinkMap { size_t bias; const char *name; void *dynamic; struct LinkMap *next, *previous; };
struct DlInfo { const char *name; void *base; const char *symbol; void *address; };
extern int dladdr(void *, struct DlInfo *);
static void require(int ok, const char *message) {
    if (ok) return;
    size_t n = 0; while (message[n]) ++n;
    write(2, message, n); write(2, "\n", 1); _exit(92);
}
static void check_loader_owner(void *libc, void *libdl) {
    void *loader = dlopen("ld-linux-x86-64.so.2", 2);
    require(loader != 0, "open physical loader");
    void *lookup = dlsym(loader, "dlsym");
    require(lookup && lookup == dlsym(libc, "dlsym") && lookup == dlsym(libdl, "dlsym"),
            "loader aliases must reach one physical implementation");
    struct LinkMap *map = 0;
    struct DlInfo info = {0};
    require(!dlinfo(loader, 2, &map) && map && dladdr(lookup, &info) &&
            info.base == (void *)map->bias, "dladdr must identify the actual loader image");
    require(!dlclose(loader), "release physical loader reference");
}

static void check_dso(void *dso, int expected) {
    int (*value)(void) = dlsym(dso, "loader_dso_value");
    int *(*tls)(void) = dlsym(dso, "loader_dso_tls");
    struct DlInfo info = {0};
    require(value && tls && value() == 20 && *tls() == expected,
            "ELF DSO constructor and TLS must survive fork");
    require(dladdr((void *)value, &info) && info.base &&
            ((const unsigned char *)info.base)[0] == 0x7f &&
            ((const unsigned char *)info.base)[1] == 'E', "DSO must retain real ELF mapping");
    ++*tls();
}
static void check(void *libc, void *libdl) {
    check_loader_owner(libc, libdl);
    lookup_fn c_lookup = dlsym(libc, "dlsym"), dl_lookup = dlsym(libdl, "dlsym");
    require(c_lookup && dl_lookup, "explicit dlsym entry missing");
    void *expected = dlsym(libc, "write");
    require(expected != 0, "libc write missing");
    require(dl_lookup((void *)-1L, "write") == expected,
            "libdl RTLD_NEXT lost the guest caller");
    require(c_lookup((void *)-1L, "write") == expected,
            "libc RTLD_NEXT lost the guest caller");
    version_lookup_fn c_version = dlsym(libc, "dlvsym"), dl_version = dlsym(libdl, "dlvsym");
    require(c_version && dl_version, "explicit dlvsym entry missing");
    require(c_version((void *)-1L, "write", "GLIBC_2.2.5") == expected &&
            dl_version((void *)-1L, "write", "GLIBC_2.2.5") == expected,
            "versioned RTLD_NEXT lost the guest caller");
    const char *names[] = {"dlopen", "dlsym", "dlvsym", "dlclose", "dlerror",
                          "dladdr", "dlinfo", "dl_iterate_phdr"};
    for (unsigned i = 0; i < sizeof(names) / sizeof(names[0]); ++i) {
        /* All entry points share the canonical native loader implementation. */
        require(dlsym((void *)0, names[i]) && dlsym(libc, names[i]) &&
                dlsym(libdl, names[i]), names[i]);
    }
    require(dlvsym(libdl, "dlsym", "GLIBC_2.2.5") == (void *)dl_lookup,
            "versioned loader alias differs");
    require(!dl_version(libdl, "dlsym", "GLIBC_99.99"), "unknown loader version accepted");
    require(!dl_lookup(libdl, "kinakaze_missing_loader_symbol"), "missing lookup succeeded");
    const char *(*c_error)(void) = dlsym(libc, "dlerror");
    /* Successful lookup clears the previous error; trigger through the explicit entry again. */
    require(!dl_lookup(libdl, "kinakaze_missing_loader_symbol"), "missing lookup succeeded twice");
    require(c_error() && !dlerror(), "loader error state is not shared and consumed once");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    void *libc = dlopen("libc.so.6", 2), *libdl = dlopen("libdl.so.2", 2);
    require(libc && libdl, "open loader ABI providers");
    check(libc, libdl);
    void *dso = dlopen("/loader-entry-dso.so", 2);
    require(dso != 0, "open real ELF DSO beside PE modules");
    check_dso(dso, 31);
    struct LinkMap *map = 0;
    require(!dlinfo(libc, 2, &map) && map && map->name, "parent link_map");
    int child = fork(); require(child >= 0, "fork loader entries");
    if (!child) {
        check(libc, libdl);
        check_dso(dso, 32);
        struct LinkMap *restored = 0;
        require(!dlinfo(libc, 2, &restored) && restored == map && map->name[0],
                "fork preserves link_map and name addresses");
        void *math = dlopen("libm.so.6", 2);
        require(math && dlsym(math, "cos") && !dlclose(math), "child open and close native module");
        int grandchild = fork(); require(grandchild >= 0, "second generation loader fork");
        if (!grandchild) {
            check(libc, libdl);
            check_dso(dso, 33);
            require(!dlclose(dso), "grandchild release ELF DSO");
            require(!dlinfo(libc, 2, &restored) && restored == map && map->name[0], "grandchild link_map");
            require(!dlclose(libdl) && !dlclose(libc), "grandchild release inherited handles");
            _exit(0);
        }
        int status = -1;
        require(waitpid(grandchild, &status, 0) == grandchild && !status, "grandchild loader entries");
        check_dso(dso, 33);
        require(!dlclose(dso), "child release ELF DSO");
        require(!dlclose(libdl) && !dlclose(libc), "child release inherited handles");
        _exit(0);
    }
    int status = -1;
    require(waitpid(child, &status, 0) == child && status == 0, "child loader entries");
    check(libc, libdl);
    check_dso(dso, 32);
    require(!dlclose(dso), "parent release ELF DSO");
    require(!dlclose(libdl) && !dlclose(libc), "close loader ABI references");
    write(1, "LOADER_ENTRY_OK\n", 15); _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");
}
