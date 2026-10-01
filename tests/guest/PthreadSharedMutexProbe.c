/* Linux ABI callers, shared mappings, alias addresses and real fork children. */
typedef unsigned long word;
struct timespec { long seconds, nanoseconds; };
struct shared { word mutex[5]; long counter; };
extern int pthread_mutexattr_init(void *);
extern int pthread_mutexattr_destroy(void *);
extern int pthread_mutexattr_settype(void *, int);
extern int pthread_mutexattr_gettype(const void *, int *);
extern int pthread_mutexattr_setrobust(void *, int);
extern int pthread_mutexattr_getrobust(const void *, int *);
extern int pthread_mutexattr_setpshared(void *, int);
extern int pthread_mutexattr_getpshared(const void *, int *);
extern int pthread_mutexattr_setprotocol(void *, int);
extern int pthread_mutexattr_getprotocol(const void *, int *);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_trylock(void *);
extern int pthread_mutex_timedlock(void *, const struct timespec *);
extern int pthread_mutex_clocklock(void *, int, const struct timespec *);
extern int pthread_mutex_unlock(void *);
extern int pthread_mutex_consistent(void *);
extern int pthread_create(word *, const void *, void *(*)(void *), void *);
extern int pthread_join(word, void **);
extern int pthread_cancel(word);
extern void pthread_testcancel(void);
extern void pthread_exit(void *) __attribute__((noreturn));
extern int clock_gettime(int, struct timespec *);
extern int sched_yield(void);
extern int usleep(unsigned int);
extern void *mmap(void *, word, int, int, int, long);
extern int munmap(void *, word);
extern int open(const char *, int, ...);
extern int ftruncate(int, long);
extern int unlink(const char *);
extern int pipe(int *);
extern long read(int, void *, word);
extern long write(int, const void *, word);
extern int close(int);
extern int fork(void);
extern int waitpid(int, int *, int);
extern long syscall(long, ...);
extern void _exit(int) __attribute__((noreturn));

static int initialize(struct shared *s, int kind, int robust) {
    int attr, found;
    if (pthread_mutexattr_init(&attr)) return 1;
    if (pthread_mutexattr_setpshared(&attr, 1)) return 2;
    if (pthread_mutexattr_setrobust(&attr, robust) || pthread_mutexattr_settype(&attr, kind)) return 3;
    if (pthread_mutexattr_getpshared(&attr, &found) || found != 1) return 4;
    if (pthread_mutexattr_getrobust(&attr, &found) || found != robust) return 5;
    if (pthread_mutexattr_gettype(&attr, &found) || found != kind) return 6;
#ifdef PROBE_PI_MUTEX
    if (pthread_mutexattr_setprotocol(&attr, 1) ||
        pthread_mutexattr_getprotocol(&attr, &found) || found != 1) return 66;
#endif
    s->counter = 0;
    int result = pthread_mutex_init(s->mutex, &attr);
    pthread_mutexattr_destroy(&attr);
    return result ? 7 : 0;
}
static int joined(int pid) {
    int status = -1;
    if (waitpid(pid, &status, 0) != pid) return 8;
    return status ? 100 + ((status >> 8) & 255) : 0;
}
static struct timespec deadline(int clock, int milliseconds) {
    struct timespec t;
    clock_gettime(clock, &t);
    t.nanoseconds += (long)milliseconds * 1000000;
    t.seconds += t.nanoseconds / 1000000000;
    t.nanoseconds %= 1000000000;
    return t;
}
static int increment(struct shared *s, int count) {
    for (int i = 0; i < count; ++i) {
        if (pthread_mutex_lock(s->mutex)) return 9;
        long value = s->counter;
        if ((i & 31) == 0) sched_yield();
        s->counter = value + 1;
        if (pthread_mutex_unlock(s->mutex)) return 10;
    }
    return 0;
}

int probe_shared_types(int kind, int robust) {
    struct shared *s = mmap(0, 4096, 3, 33, -1, 0);
    if (s == (void *)-1) return 11;
    int result = initialize(s, kind, robust);
    if (result) return result;
    if (pthread_mutex_lock(s->mutex)) return 12;
    int child = fork();
    if (child < 0) return 13;
    if (!child) {
        /* A recursive owner in the parent is not the child's owner. */
        if (pthread_mutex_trylock(s->mutex) != 16) _exit(14);
        if (pthread_mutex_unlock(s->mutex) != 1) _exit(15);
        if (pthread_mutex_consistent(s->mutex) != 22) _exit(16);
        _exit(0);
    }
    if ((result = joined(child))) return result;
    if (kind == 1) {
        if (pthread_mutex_trylock(s->mutex) || pthread_mutex_unlock(s->mutex)) return 17;
    } else if (pthread_mutex_trylock(s->mutex) != 16) return 18;
    if (pthread_mutex_unlock(s->mutex)) return 19;
    child = fork();
    if (child < 0) return 20;
    if (!child) _exit(increment(s, 800));
    if ((result = increment(s, 800))) return result;
    if ((result = joined(child))) return result;
    if (s->counter != 1600) return 21;
    if (pthread_mutex_destroy(s->mutex)) return 22;
    return munmap(s, 4096) ? 23 : 0;
}

int probe_shared_timed(int clock) {
    struct shared *s = mmap(0, 4096, 3, 33, -1, 0);
    if (s == (void *)-1) return 24;
    int result = initialize(s, 0, 0), ready[2], release[2];
    if (result) return result;
    if (pipe(ready) || pipe(release)) return 25;
    int child = fork();
    if (child < 0) return 26;
    if (!child) {
        char byte = 1;
        if (pthread_mutex_lock(s->mutex)) _exit(27);
        if (write(ready[1], &byte, 1) != 1 || read(release[0], &byte, 1) != 1) _exit(28);
        _exit(pthread_mutex_unlock(s->mutex) ? 29 : 0);
    }
    char byte;
    if (read(ready[0], &byte, 1) != 1) return 30;
    struct timespec t = deadline(clock, 25);
    if (pthread_mutex_clocklock(s->mutex, clock, &t) != 110) return 31;
    if (write(release[1], &byte, 1) != 1) return 32;
    t = deadline(clock, 5000);
    if (pthread_mutex_clocklock(s->mutex, clock, &t) || pthread_mutex_unlock(s->mutex)) return 33;
    if ((result = joined(child))) return result;
    for (int i = 0; i != 2; ++i) { close(ready[i]); close(release[i]); }
    if (pthread_mutex_destroy(s->mutex)) return 34;
    return munmap(s, 4096) ? 35 : 0;
}

static struct shared *thread_mutex;
static int thread_mode, thread_ready;
static void *owner(void *unused) {
    (void)unused;
    int result = pthread_mutex_lock(thread_mutex->mutex);
    if (result) return (void *)(word)(1000 + result);
    __atomic_store_n(&thread_ready, 1, __ATOMIC_RELEASE);
    if (thread_mode == 1) pthread_exit((void *)42);
    if (thread_mode == 2) syscall(60, 0);
    if (thread_mode == 3) for (;;) { pthread_testcancel(); usleep(1000); }
    return (void *)42;
}
int probe_shared_death(int kind, int mode, int poison, int robust) {
    struct shared *s = mmap(0, 4096, 3, 33, -1, 0);
    if (s == (void *)-1) return 36;
    int result = initialize(s, kind, robust);
    if (result) return result;
    int child = fork();
    if (child < 0) return 37;
    if (!child) {
        if (mode == 4) {
            if (pthread_mutex_lock(s->mutex)) _exit(38);
            _exit(0); /* Whole process exits with ownership. */
        }
        thread_mutex = s; thread_mode = mode; thread_ready = 0;
        word thread;
        if (pthread_create(&thread, 0, owner, 0)) _exit(39);
        if (mode == 3) {
            while (!__atomic_load_n(&thread_ready, __ATOMIC_ACQUIRE)) sched_yield();
            if (pthread_cancel(thread)) _exit(40);
        }
        void *value;
        if (pthread_join(thread, &value)) _exit(41);
        if (mode < 2 && value != (void *)42) _exit(42);
        if (mode == 2 && value != 0) _exit(43);
        if (mode == 3 && value != (void *)~(word)0) _exit(44);
        _exit(0);
    }
    if ((result = joined(child))) return result;
    if (!robust) {
        struct timespec t = deadline(0, 20);
        if (pthread_mutex_trylock(s->mutex) != 16 || pthread_mutex_timedlock(s->mutex, &t) != 110) return 45;
        return munmap(s, 4096) ? 46 : 0;
    }
    if (pthread_mutex_lock(s->mutex) != 130) return 47;
    if (!poison && pthread_mutex_consistent(s->mutex)) return 48;
    if (pthread_mutex_unlock(s->mutex)) return 49;
    child = fork();
    if (child < 0) return 50;
    if (!child) {
        if (pthread_mutex_trylock(s->mutex) != (poison ? 131 : 0)) _exit(51);
        if (!poison && pthread_mutex_unlock(s->mutex)) _exit(52);
        _exit(0);
    }
    if ((result = joined(child))) return result;
    if (pthread_mutex_destroy(s->mutex)) return 53;
    return munmap(s, 4096) ? 54 : 0;
}

int probe_shared_alias(const char *path) {
    int fd = open(path, 578, 0600);
    if (fd < 0 || ftruncate(fd, 4096)) return 55;
    struct shared *a = mmap(0, 4096, 3, 1, fd, 0);
    struct shared *b = mmap(0, 4096, 3, 1, fd, 0);
    if (a == (void *)-1 || b == (void *)-1 || a == b) return 56;
    int result = initialize(a, 1, 1);
    if (result) return result;
    if (pthread_mutex_lock(a->mutex) || pthread_mutex_trylock(b->mutex)) return 57;
    if (pthread_mutex_unlock(a->mutex) || pthread_mutex_unlock(b->mutex)) return 58;
    int child = fork();
    if (child < 0) return 59;
    if (!child) _exit(increment(b, 600));
    if ((result = increment(a, 600))) return result;
    if ((result = joined(child))) return result;
    if (a->counter != 1200 || b->counter != 1200) return 60;
    word old_generation = a->mutex[4];
    if (pthread_mutex_destroy(b->mutex) || initialize(a, 0, 0)) return 61;
#ifndef PROBE_PI_MUTEX
    if (old_generation == b->mutex[4]) return 62;
#else
    (void)old_generation;
    if (((int *)b->mutex)[4] != (32 | 128)) return 62;
#endif
    if (pthread_mutex_lock(b->mutex) || pthread_mutex_unlock(a->mutex)) return 63;
    if (pthread_mutex_destroy(a->mutex)) return 64;
    if (munmap(a, 4096) || munmap(b, 4096) || close(fd) || unlink(path)) return 65;
    return 0;
}
