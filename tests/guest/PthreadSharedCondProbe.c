/* Shared conditions with real fork processes, file aliases and robust mutexes. */
typedef unsigned long word;
struct timespec { long seconds, nanoseconds; };
struct shared { word mutex[5], cond[2][6]; int turn, count, ready, done, permits, stop, repaired; };
extern int pthread_mutexattr_init(void *);
extern int pthread_mutexattr_setpshared(void *, int);
extern int pthread_mutexattr_setrobust(void *, int);
extern int pthread_mutexattr_settype(void *, int);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_trylock(void *);
extern int pthread_mutex_unlock(void *);
extern int pthread_mutex_consistent(void *);
extern int pthread_condattr_init(void *);
extern int pthread_condattr_setpshared(void *, int);
extern int pthread_condattr_getpshared(const void *, int *);
extern int pthread_condattr_setclock(void *, int);
extern int pthread_condattr_getclock(const void *, int *);
extern int pthread_cond_init(void *, const void *);
extern int pthread_cond_destroy(void *);
extern int pthread_cond_signal(void *);
extern int pthread_cond_broadcast(void *);
extern int pthread_cond_timedwait(void *, void *, const struct timespec *);
extern int clock_gettime(int, struct timespec *);
extern void *mmap(void *, word, int, int, int, long);
extern int munmap(void *, word);
extern int open(const char *, int, ...);
extern int ftruncate(int, long);
extern int unlink(const char *);
extern int close(int);
extern int fork(void);
extern int waitpid(int, int *, int);
extern void _exit(int) __attribute__((noreturn));

static int initialize(struct shared *s, int clock) {
    int attr, found;
    if (pthread_mutexattr_init(&attr) || pthread_mutexattr_setpshared(&attr, 1) ||
        pthread_mutexattr_setrobust(&attr, 1) || pthread_mutexattr_settype(&attr, 2) ||
        pthread_mutex_init(s->mutex, &attr)) return 1;
    if (pthread_condattr_init(&attr) || pthread_condattr_setclock(&attr, clock) ||
        pthread_condattr_setpshared(&attr, 1)) return 2;
    if (pthread_condattr_getclock(&attr, &found) || found != clock ||
        pthread_condattr_getpshared(&attr, &found) || found != 1) return 3;
    if (attr != ((clock << 1) | 1)) return 4;
    if (pthread_cond_init(s->cond[0], &attr) || pthread_cond_init(s->cond[1], &attr)) return 5;
    s->turn = s->count = s->ready = s->done = s->permits = s->stop = s->repaired = 0;
    return 0;
}
static struct timespec deadline(int clock) {
    struct timespec time;
    clock_gettime(clock, &time);
    time.seconds += 10;
    return time;
}
static int joined(int child) {
    int status;
    if (waitpid(child, &status, 0) != child) return 6;
    return status ? 100 + ((status >> 8) & 255) : 0;
}
static int destroy(struct shared *s) {
    return pthread_cond_destroy(s->cond[0]) || pthread_cond_destroy(s->cond[1]) ||
           pthread_mutex_destroy(s->mutex) ? 7 : 0;
}
static int rounds(struct shared *s, int side, int clock, int iterations) {
    if (pthread_mutex_lock(s->mutex)) return 8;
    struct timespec time = deadline(clock);
    for (int i = 0; i < iterations; ++i) {
        while (s->turn != side) {
            int result = pthread_cond_timedwait(s->cond[side], s->mutex, &time);
            if (result) return 20 + result;
            if (pthread_mutex_trylock(s->mutex) != 16) return 9;
        }
        ++s->count;
        s->turn = 1 - side;
        if (pthread_cond_signal(s->cond[1 - side])) return 10;
    }
    return pthread_mutex_unlock(s->mutex) ? 11 : 0;
}
long long probe_shared_cond_handoff(int clock, int iterations, const char *path) {
    int fd = -1;
    struct shared *a, *b;
    if (path) {
        fd = open(path, 578, 0600);
        if (fd < 0 || ftruncate(fd, 4096)) return -12;
        a = mmap(0, 4096, 3, 1, fd, 0);
        b = mmap(0, 4096, 3, 1, fd, 0);
    } else {
        a = mmap(0, 4096, 3, 33, -1, 0);
        b = a;
    }
    if (a == (void *)-1 || b == (void *)-1 || (path && a == b)) return -13;
    int result = initialize(a, clock);
    if (result) return -result;
    struct timespec start, end;
    clock_gettime(1, &start);
    int child = fork();
    if (child < 0) return -14;
    if (!child) _exit(rounds(b, 1, clock, iterations));
    result = rounds(a, 0, clock, iterations);
    if (result) return -result;
    if ((result = joined(child))) return -result;
    clock_gettime(1, &end);
    if (a->count != 2 * iterations) return -15;
    if ((result = destroy(a))) return -result;
    if (munmap(a, 4096) || (path && (munmap(b, 4096) || close(fd) || unlink(path)))) return -16;
    return (end.seconds - start.seconds) * 1000000000LL + end.nanoseconds - start.nanoseconds;
}

int probe_shared_cond_broadcast(int clock, int waiters) {
    struct shared *s = mmap(0, 4096, 3, 33, -1, 0);
    if (s == (void *)-1) return 17;
    int result = initialize(s, clock), children[8];
    if (result) return result;
    if (waiters < 2 || waiters > 8) return 18;
    for (int i = 0; i < waiters; ++i) {
        int child = fork();
        if (child < 0) return 19;
        if (!child) {
            if (pthread_mutex_lock(s->mutex)) _exit(20);
            ++s->ready;
            if (pthread_cond_signal(s->cond[1])) _exit(21);
            struct timespec time = deadline(clock);
            while (!s->stop && !s->permits) {
                if (pthread_cond_timedwait(s->cond[0], s->mutex, &time)) _exit(22);
            }
            if (!s->stop) --s->permits;
            ++s->done;
            if (pthread_cond_signal(s->cond[1]) || pthread_mutex_unlock(s->mutex)) _exit(23);
            _exit(0);
        }
        children[i] = child;
    }
    if (pthread_mutex_lock(s->mutex)) return 24;
    struct timespec time = deadline(clock);
    while (s->ready != waiters) {
        if (pthread_cond_timedwait(s->cond[1], s->mutex, &time)) return 25;
    }
    s->permits = 1;
    if (pthread_cond_signal(s->cond[0])) return 26;
    while (!s->done) {
        if (pthread_cond_timedwait(s->cond[1], s->mutex, &time)) return 27;
    }
    if (s->done != 1) return 28;
    s->stop = 1;
    if (pthread_cond_broadcast(s->cond[0]) || pthread_mutex_unlock(s->mutex)) return 29;
    for (int i = 0; i < waiters; ++i) if ((result = joined(children[i]))) return result;
    if (s->done != waiters) return 30;
    if ((result = destroy(s))) return result;
    return munmap(s, 4096) ? 31 : 0;
}

int probe_shared_cond_owner_death(int clock) {
    struct shared *s = mmap(0, 4096, 3, 33, -1, 0);
    if (s == (void *)-1) return 32;
    int result = initialize(s, clock);
    if (result) return result;
    int waiter = fork();
    if (waiter < 0) return 33;
    if (!waiter) {
        if (pthread_mutex_lock(s->mutex)) _exit(34);
        s->ready = 1;
        if (pthread_cond_signal(s->cond[1])) _exit(35);
        struct timespec time = deadline(clock);
        if (pthread_cond_timedwait(s->cond[0], s->mutex, &time) != 130) _exit(36);
        if (pthread_mutex_consistent(s->mutex)) _exit(37);
        s->repaired = 1;
        if (pthread_mutex_unlock(s->mutex)) _exit(38);
        _exit(0);
    }
    if (pthread_mutex_lock(s->mutex)) return 39;
    struct timespec time = deadline(clock);
    while (!s->ready) {
        if (pthread_cond_timedwait(s->cond[1], s->mutex, &time)) return 40;
    }
    if (pthread_mutex_unlock(s->mutex)) return 41;
    int waker = fork();
    if (waker < 0) return 42;
    if (!waker) {
        if (pthread_mutex_lock(s->mutex) || pthread_cond_signal(s->cond[0])) _exit(43);
        _exit(0); /* Abandon after committed signal, before mutex unlock. */
    }
    if ((result = joined(waker)) || (result = joined(waiter))) return result;
    if (!s->repaired) return 44;
    if ((result = destroy(s))) return result;
    return munmap(s, 4096) ? 45 : 0;
}
