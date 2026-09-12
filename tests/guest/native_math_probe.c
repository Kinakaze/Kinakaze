/* Real Linux ELF calls the independently loaded PE math module. */
typedef unsigned long size_t;
typedef unsigned long pthread_t;
extern long write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));
extern int *__errno_location(void);
extern void *dlopen(const char *, int);
extern void *dlsym(void *, const char *);
extern void *dlvsym(void *, const char *, const char *);
extern int dlclose(void *);
extern int fork(void);
extern int waitpid(int, int *, int);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
struct DlInfo { const char *name; void *base; const char *symbol; void *address; };
extern int dladdr(void *, struct DlInfo *);

static void require(int good, const char *message) {
    if (good) return;
    size_t length = 0;
    while (message[length]) ++length;
    write(2, message, length); write(2, "\n", 1); _exit(1);
}
static long double (*scale)(long double, int);
static double (*cosine)(double);
static int *main_errno;

static void *thread(void *unused) {
    (void)unused;
    require(__errno_location() != main_errno, "math thread has independent errno");
    *__errno_location() = 83;
    volatile long double result = scale(1.0L, 20000);
    require(result > 1.0L && *__errno_location() == 34, "math range error reaches libc TLS");
    require(cosine(0.0) == 1.0, "math thread call");
    return (void *)19;
}

__attribute__((noreturn)) void probe_start(void) {
    void *module = dlopen("libm.so.6", 2);
    require(module != 0, "open native SONAME");
    cosine = dlsym(module, "cos");
    scale = dlsym(module, "scalbnl");
    double (*copy_sign)(double, double) = dlsym(module, "copysign");
    require(cosine && scale && copy_sign, "resolve native math addresses");
    struct DlInfo info = {0};
    require(dladdr((void *)cosine, &info) != 0 && info.base,
            "native PE address is owned by linker");
    const unsigned char *image = info.base;
    require(image[0] == 'M' && image[1] == 'Z', "guest calls the PE, without ELF trampoline");
    require(dlvsym(module, "cos", "GLIBC_2.2.5") == (void *)cosine,
            "versioned lookup preserves direct address");
    require(dlvsym(module, "cos", "GLIBC_99.99") == 0, "unknown ABI version rejected");
    union { double number; unsigned long bits; } sign = { .number = copy_sign(0.0, -1.0) };
    require(sign.bits == (1UL << 63), "negative zero sign bit");
    require(scale(1.5L, 3) == 12.0L, "System V x87 long double ABI");
    require(cosine(0.0) == 1.0, "native cosine");
    main_errno = __errno_location(); *main_errno = 71;
    pthread_t id; void *result = 0;
    require(pthread_create(&id, 0, thread, 0) == 0 && pthread_join(id, &result) == 0
            && result == (void *)19 && *main_errno == 71, "thread math and errno isolation");
    int child = fork(); require(child >= 0, "fork native module");
    if (!child) {
        require(cosine(0.0) == 1.0 && scale(1.5L, 3) == 12.0L, "restored native addresses");
        *__errno_location() = 0;
        volatile long double overflow = scale(1.0L, 20000);
        require(overflow > 1.0L && *__errno_location() == 34, "restored math errno import");
        pthread_t child_thread; void *child_result = 0;
        require(!pthread_create(&child_thread, 0, thread, 0) &&
                !pthread_join(child_thread, &child_result) && child_result == (void *)19 &&
                *__errno_location() == 34, "child thread math and errno isolation");
        int nested = fork(); require(nested >= 0, "second generation math fork");
        if (!nested) {
            require(*__errno_location() == 34 && cosine(0.0) == 1.0, "grandchild math state");
            *__errno_location() = 29;
            _exit(0);
        }
        int nested_status = -1;
        require(waitpid(nested, &nested_status, 0) == nested && !nested_status &&
                *__errno_location() == 34, "nested math errno remains private");
        _exit(0);
    }
    int status = -1;
    require(waitpid(child, &status, 0) == child && status == 0, "native math fork child success");
    require(*main_errno == 71 && cosine(0.0) == 1.0, "parent survives child module cleanup");
    require(dlclose(module) == 0, "release native math reference");
    write(1, "NATIVE_MATH_OK\n", 15); _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");
}
