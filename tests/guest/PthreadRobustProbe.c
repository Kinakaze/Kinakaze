/* Real Linux callers: owner return/exit/cancel/raw exit, repair and fork. */
typedef unsigned long word;
struct timespec { long seconds, nanoseconds; };
extern int pthread_create(word *, const void *, void *(*)(void *), void *);
extern int pthread_join(word, void **);
extern int pthread_cancel(word);
extern void pthread_testcancel(void);
extern void pthread_exit(void *) __attribute__((noreturn));
extern int pthread_key_create(unsigned int *, void (*)(void *));
extern int pthread_key_delete(unsigned int);
extern int pthread_setspecific(unsigned int, const void *);
extern int __cxa_thread_atexit_impl(void (*)(void *), void *, void *);
extern int pthread_mutexattr_init(void *);
extern int pthread_mutexattr_settype(void *, int);
extern int pthread_mutexattr_gettype(const void *, int *);
extern int pthread_mutexattr_setrobust(void *, int);
extern int pthread_mutexattr_getrobust(const void *, int *);
extern int pthread_mutexattr_destroy(void *);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_trylock(void *);
extern int pthread_mutex_timedlock(void *, const struct timespec *);
extern int pthread_mutex_unlock(void *);
extern int pthread_mutex_consistent(void *);
extern int pthread_mutex_consistent_np(void *);
extern int clock_gettime(int, struct timespec *);
extern int usleep(unsigned int);
extern int sched_yield(void);
extern long syscall(long, ...);
extern int fork(void);
extern int waitpid(int, int *, int);
extern void _exit(int) __attribute__((noreturn));
static word mutex[5];
static int ready, kind, mode, expected;
static unsigned int thread_key;
static int destructor_called;
static int cxx_called;
static void destructor(void *value) { if (value) destructor_called = 1; }
static void cxx_destructor(void *value) { if (value) cxx_called = 1; }

static int initialize(int type) {
    int attr, found;
    if (pthread_mutexattr_init(&attr) || pthread_mutexattr_setrobust(&attr, 1)) return 1;
    if (pthread_mutexattr_settype(&attr, type)) return 2;
    if (pthread_mutexattr_gettype(&attr, &found) || found != type) return 3;
    if (pthread_mutexattr_getrobust(&attr, &found) || found != 1) return 4;
    int result = pthread_mutex_init(mutex, &attr);
    pthread_mutexattr_destroy(&attr);
    kind = type;
    return result ? 5 : 0;
}

static void *owner(void *unused) {
    (void)unused;
    int result = pthread_mutex_lock(mutex);
    if (result != expected) return (void *)(word)(1000 + result);
    if (kind == 1 && pthread_mutex_lock(mutex)) return (void *)1001;
    if (pthread_setspecific(thread_key, (void *)1)) return (void *)1002;
    if (__cxa_thread_atexit_impl(cxx_destructor, (void *)1, mutex)) return (void *)1003;
    __atomic_store_n(&ready, 1, __ATOMIC_RELEASE);
    if (mode == 1) pthread_exit((void *)0x42);
    if (mode == 2) syscall(60, 0); /* Native SYS_exit, without pthread cleanup. */
    if (mode == 3) for (;;) { pthread_testcancel(); usleep(1000); }
    return (void *)0x42;
}

static int abandon(int exit_mode, int result) {
    ready = 0;
    mode = exit_mode;
    expected = result;
    destructor_called = 0;
    cxx_called = 0;
    if (pthread_key_create(&thread_key, destructor)) return 34;
    word thread;
    if (pthread_create(&thread, 0, owner, 0)) return 6;
    if (exit_mode == 3) {
        while (!__atomic_load_n(&ready, __ATOMIC_ACQUIRE)) sched_yield();
        if (pthread_cancel(thread)) return 7;
    }
    void *value;
    if (pthread_join(thread, &value)) return 8;
    if (exit_mode == 2 && value != 0) return 37;
    if (pthread_key_delete(thread_key)) return 35;
    if (destructor_called != (exit_mode != 2) || cxx_called != (exit_mode != 2)) return 36;
    if (exit_mode == 3 && value != (void *)~(word)0) return 9;
    if (exit_mode != 2 && exit_mode != 3 && value != (void *)0x42) return 10;
    return 0;
}

int probe_robust_recovery(int type, int exit_mode, int poison) {
    int result = initialize(type);
    if (result) return result;
    if ((result = abandon(exit_mode, 0))) return result;
    /* A recovery owner dying without unlock leaves the mutex recoverable. */
    if ((result = abandon(0, 130))) return result;
    if (pthread_mutex_trylock(mutex) != 130) return 11;
    if (!poison) {
        if (pthread_mutex_consistent_np(mutex) || pthread_mutex_consistent(mutex) != 22) return 12;
        if (type == 1) {
            if (pthread_mutex_lock(mutex) || pthread_mutex_unlock(mutex)) return 13;
        } else if (pthread_mutex_trylock(mutex) != 16) return 14;
    }
    if (pthread_mutex_unlock(mutex)) return 15;
    if (poison) {
        struct timespec invalid = {0, -1};
        if (pthread_mutex_lock(mutex) != 131 || pthread_mutex_trylock(mutex) != 131 ||
            pthread_mutex_timedlock(mutex, &invalid) != 131) return 16;
        if (pthread_mutex_unlock(mutex) != 1 || pthread_mutex_consistent(mutex) != 22) return 17;
    } else {
        if (pthread_mutex_lock(mutex) || pthread_mutex_unlock(mutex)) return 18;
    }
    return pthread_mutex_destroy(mutex) ? 19 : 0;
}

int probe_robust_fork(int phase) {
    int result = initialize(1);
    if (result) return result;
    if (phase >= 2) {
        if ((result = abandon(0, 0))) return result;
        if (pthread_mutex_lock(mutex) != 130) return 20;
        if (phase == 3 && pthread_mutex_unlock(mutex)) return 21;
    } else if (phase == 1) {
        if (pthread_mutex_lock(mutex) || pthread_mutex_lock(mutex)) return 22;
    }
    int child = fork();
    if (child < 0) return 23;
    if (!child) {
        if (phase == 3) {
            if (pthread_mutex_trylock(mutex) != 131) _exit(24);
        } else {
            if (phase == 2 && pthread_mutex_consistent(mutex)) _exit(25);
            if (phase == 1 && (pthread_mutex_trylock(mutex) || pthread_mutex_unlock(mutex) ||
                              pthread_mutex_unlock(mutex) || pthread_mutex_unlock(mutex))) _exit(26);
            if (phase == 2 && pthread_mutex_unlock(mutex)) _exit(27);
            if ((result = abandon(2, 0))) _exit(result);
            if (pthread_mutex_lock(mutex) != 130 || pthread_mutex_consistent(mutex) ||
                pthread_mutex_unlock(mutex)) _exit(28);
        }
        _exit(pthread_mutex_destroy(mutex) ? 29 : 0);
    }
    int status;
    if (waitpid(child, &status, 0) != child) return 30;
    if (phase == 1 && (pthread_mutex_unlock(mutex) || pthread_mutex_unlock(mutex))) return 31;
    if (phase == 2 && (pthread_mutex_consistent(mutex) || pthread_mutex_unlock(mutex))) return 32;
    if (pthread_mutex_destroy(mutex)) return 33;
    return status ? 100 + ((status >> 8) & 255) : 0;
}
