/* A real ELF executable replacing malloc: pointers carry our own header.
 * RTLD_NEXT deliberately exercises the base implementation without recursion. */
typedef unsigned long size_t;
typedef long ssize_t;
typedef unsigned long pthread_t;
extern void *dlsym(void *, const char *);
extern void _exit(int) __attribute__((noreturn));
extern ssize_t write(int, const void *, size_t);
extern int *__errno_location(void);
extern void *memcpy(void *, const void *, size_t);
extern void *memset(void *, int, size_t);
extern char *strdup(const char *);
extern char *strndup(const char *, size_t);
extern char *getcwd(char *, size_t);
extern char *realpath(const char *, char *);
extern int asprintf(char **, const char *, ...);
extern void *fopen(const char *, const char *);
extern int fclose(void *);
extern long getline(char **, size_t *, void *);
extern long getdelim(char **, size_t *, int, void *);
extern void *open_memstream(char **, size_t *);
extern size_t fwrite(const void *, size_t, size_t, void *);
extern int fflush(void *);
extern void *reallocarray(void *, size_t, size_t);
extern int argz_append(char **, size_t *, const char *, size_t);
extern int scandir(const char *, void ***, void *, void *);
extern int fork(void);
extern int waitpid(int, int *, int);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
extern int pthread_atfork(void (*)(void), void (*)(void), void (*)(void));
extern int snd_card_get_name(int, char **);

struct Header { size_t magic, size; };
#define MAGIC 0x4d414c4c4f434f57UL
static void *(*base_malloc)(size_t);
static void *(*base_realloc)(void *, size_t);
static void (*base_free)(void *);
static unsigned allocations, releases, fail_next, child_hook;
static unsigned allocator_resolutions, fork_resolutions;
void *malloc(size_t size);
static void bases(void) {
    if (base_malloc) return;
    base_malloc = dlsym((void *)-1L, "malloc");
    base_realloc = dlsym((void *)-1L, "realloc");
    base_free = dlsym((void *)-1L, "free");
    if (!base_malloc || !base_realloc || !base_free) _exit(90);
}
static void *allocate(size_t size) {
    bases();
    if (size > (size_t)-1 - sizeof(struct Header)) return 0;
    struct Header *h = base_malloc(size + sizeof(*h));
    if (!h) return 0;
    h->magic = MAGIC; h->size = size;
    __atomic_add_fetch(&allocations, 1, __ATOMIC_RELAXED);
    return h + 1;
}
#ifdef ALLOCATOR_IFUNC
static void *(*resolve_malloc(void))(size_t) {
    ++allocator_resolutions;
    return allocate;
}
void *malloc(size_t size) __attribute__((ifunc("resolve_malloc")));
#else
void *malloc(size_t size) { return allocate(size); }
#endif
void free(void *p) {
    if (!p) return;
    struct Header *h = (struct Header *)p - 1;
    if (h->magic != MAGIC) {
        static const char error[] = "ALLOCATOR_OWNER_MISMATCH\n";
        write(2, error, sizeof(error) - 1);
        _exit(91);
    }
    h->magic = 0;
    __atomic_add_fetch(&releases, 1, __ATOMIC_RELAXED);
    base_free(h);
}
void *realloc(void *p, size_t size) {
    bases();
    if (fail_next) { fail_next = 0; *__errno_location() = 12; return 0; }
    if (!p) return malloc(size);
    if (!size) { free(p); return 0; }
    struct Header *h = (struct Header *)p - 1;
    if (h->magic != MAGIC) _exit(92);
    if (size > (size_t)-1 - sizeof(*h)) return 0;
    h = base_realloc(h, size + sizeof(*h));
    if (!h) return 0;
    h->size = size;
    return h + 1;
}
void *calloc(size_t n, size_t size) {
    if (size && n > (size_t)-1 / size) return 0;
    void *p = malloc(n * size);
    if (p) memset(p, 0, n * size);
    return p;
}
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
static int buffers(void) {
    char *p = strdup("interposed"); CHECK(p); free(p);
    p = strndup("hello", 3); CHECK(p && p[3] == 0); free(p);
    p = getcwd(0, 0); CHECK(p); free(p);
    p = realpath("/proc/filesystems", 0); CHECK(p); free(p);
    CHECK(asprintf(&p, "%s:%d", "buffer", 13) == 9); free(p);
    size_t cap = 0;
    void *file = fopen("/proc/filesystems", "r"); CHECK(file);
    p = 0;
    CHECK(getline(&p, &cap, file) > 0); free(p);
    CHECK(fclose(file) == 0);
    file = fopen("/proc/filesystems", "r"); CHECK(file);
    p = malloc(2); cap = 2;
    char *old = p;
    fail_next = 1;
    CHECK(getline(&p, &cap, file) == -1 && p == old && cap == 2);
    free(p); CHECK(fclose(file) == 0);
    file = fopen("/proc/filesystems", "r"); CHECK(file);
    p = malloc(2); cap = 2;
    CHECK(getdelim(&p, &cap, '\n', file) > 2); free(p);
    CHECK(fclose(file) == 0);
    size_t length = 0;
    file = open_memstream(&p, &length); CHECK(file);
    for (int i = 0; i < 64; ++i) CHECK(fwrite("abcdef", 1, 6, file) == 6);
    CHECK(fflush(file) == 0 && length == 384);
    CHECK(fclose(file) == 0 && p[length] == 0); free(p);
    p = malloc(1); p[0] = 42;
    p = reallocarray(p, 65, 32); CHECK(p && p[0] == 42); free(p);
    p = 0; length = 0;
    CHECK(argz_append(&p, &length, "alpha", 6) == 0);
    CHECK(argz_append(&p, &length, "beta", 5) == 0 && length == 11); free(p);
    void **list = 0;
    int count = scandir("/etc", &list, 0, 0); CHECK(count > 0);
    for (int i = 0; i < count; ++i) free(list[i]);
    free(list);
    CHECK(snd_card_get_name(0, &p) == 0 && p); free(p);
    return 0;
}
static void *thread(void *arg) { (void)arg; return (void *)(long)buffers(); }
static void after_fork(void) {
    if (allocator_resolutions != fork_resolutions) _exit(95);
    char *p = strdup("child hook");
    if (!p) _exit(93);
    free(p); child_hook = 1;
}
static int run(void) {
    int rc = buffers(); if (rc) return rc;
    pthread_t id; void *result;
    CHECK(pthread_create(&id, 0, thread, 0) == 0);
    CHECK(pthread_join(id, &result) == 0);
    if (result) return (int)(long)result;
    CHECK(pthread_atfork(0, 0, after_fork) == 0);
    fork_resolutions = allocator_resolutions;
    int pid = fork(); CHECK(pid >= 0);
    if (!pid) _exit(child_hook ? buffers() : 94);
    int status = -1;
    CHECK(waitpid(pid, &status, 0) == pid && status == 0);
    CHECK(allocations == releases);
    return 0;
}
__attribute__((used, noinline)) static void entry(void) {
    int rc = run();
    if (!rc) {
        static const char success[] = "ALLOCATOR_INTERPOSITION_FORK_OK\n";
        write(1, success, sizeof(success) - 1);
    }
    else {
        char message[] = "ALLOCATOR_PROBE_FAILURE line=000\n";
        size_t at = sizeof(message) - 5;
        message[at] = '0' + (rc / 100) % 10;
        message[at + 1] = '0' + (rc / 10) % 10;
        message[at + 2] = '0' + rc % 10;
        write(2, message, sizeof(message) - 1);
    }
    _exit(rc != 0);
}
__attribute__((naked)) void _start(void) {
    __asm__ volatile("andq $-16, %rsp; call entry; ud2");
}
