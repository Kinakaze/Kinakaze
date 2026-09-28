typedef unsigned long thread;
extern int pthread_create(thread *, const void *, void *(*)(void *), void *);
extern int pthread_join(thread, void **);
extern int vfork(void) __attribute__((returns_twice));
extern int fork(void) __attribute__((returns_twice));
extern int execve(const char *, char *const *, char *const *);
extern int waitpid(int, int *, int);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));
extern int pipe2(int *, int);
extern int close(int);
extern int usleep(unsigned);
static int ready;
static int stop, churn_error;

static void *churn(void *unused) {
    (void)unused;
    while (!__atomic_load_n(&stop, __ATOMIC_ACQUIRE)) {
        int pipes[8];
        for (int i = 0; i < 8; i += 2)
            if (pipe2(pipes + i, 0x80000)) {
                churn_error = *__errno_location();
                return 0;
            }
        for (int i = 0; i < 8; ++i) close(pipes[i]);
        usleep(1000);
    }
    return 0;
}

static void *run(void *argument) {
    __atomic_add_fetch(&ready, 1, __ATOMIC_SEQ_CST);
    while (__atomic_load_n(&ready, __ATOMIC_SEQ_CST) != 4) __asm__ volatile("pause");
    for (int index = 0; index < 12; ++index) {
        char *args[] = {"/bin/true", 0}, *env[] = {0};
        int child = ((unsigned long)argument & 1) ? vfork() : fork();
        if (child == 0) {
            execve(args[0], args, env);
            _exit(*__errno_location());
        }
        if (child < 0) return (void *)(unsigned long)(1000 + *__errno_location());
        int status = -1, waited;
        do { waited = waitpid(child, &status, 0); } while (waited < 0 && *__errno_location() == 4);
        if (waited != child) return (void *)(unsigned long)(2000 + *__errno_location());
        if (status != 0) return (void *)(unsigned long)(3000 + status);
    }
    return 0;
}

int probe(void) {
    thread threads[4], allocator;
    if (pthread_create(&allocator, 0, churn, 0)) _exit(90);
    int error = 0;
    for (unsigned long index = 0; index < 4; ++index)
        if (pthread_create(&threads[index], 0, run, (void *)index)) _exit(90);
    for (unsigned long index = 0; index < 4; ++index) {
        void *result = 0;
        if (pthread_join(threads[index], &result)) _exit(91);
        if (result) error = (unsigned long)result;
    }
    __atomic_store_n(&stop, 1, __ATOMIC_RELEASE);
    if (pthread_join(allocator, 0)) return 2;
    return error ? error : churn_error;
}
