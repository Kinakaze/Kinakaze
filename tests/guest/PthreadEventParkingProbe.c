/* Linux x86_64 ABI fixtures for the native pthread parking backend. */
typedef unsigned long word;
struct timespec { long seconds, nanoseconds; };
extern int pthread_create(word *, const void *, void *(*)(void *), void *);
extern int pthread_join(word, void **);
extern int pthread_kill(word, int);
extern int pthread_mutexattr_init(void *);
extern int pthread_mutexattr_settype(void *, int);
extern int pthread_mutexattr_destroy(void *);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_trylock(void *);
extern int pthread_mutex_timedlock(void *, const struct timespec *);
extern int pthread_mutex_unlock(void *);
extern int pthread_condattr_init(void *);
extern int pthread_condattr_setclock(void *, int);
extern int pthread_condattr_destroy(void *);
extern int pthread_cond_init(void *, const void *);
extern int pthread_cond_destroy(void *);
extern int pthread_cond_signal(void *);
extern int pthread_cond_timedwait(void *, void *, const struct timespec *);
extern int clock_gettime(int, struct timespec *);
extern int sched_yield(void);
extern int usleep(unsigned int);
extern void (*signal(int, void (*)(int)))(int);
extern int fork(void);
extern int waitpid(int, int *, int);
extern void _exit(int) __attribute__((noreturn));
static word mutex[5], cond[6];
static int ready, done, handled, handler_unlocked;

static long long nanoseconds(struct timespec value) {
    return (long long)value.seconds * 1000000000 + value.nanoseconds;
}

static struct timespec deadline(int clock, long milliseconds) {
    struct timespec value;
    clock_gettime(clock, &value);
    value.nanoseconds += milliseconds * 1000000;
    value.seconds += value.nanoseconds / 1000000000;
    value.nanoseconds %= 1000000000;
    return value;
}

static int initialize(void) {
    int attr, clock;
    if (pthread_mutexattr_init(&attr) || pthread_mutexattr_settype(&attr, 2)) return 1;
    if (pthread_mutex_init(mutex, &attr)) return 2;
    pthread_mutexattr_destroy(&attr);
    if (pthread_condattr_init(&clock) || pthread_condattr_setclock(&clock, 1)) return 3;
    int result = pthread_cond_init(cond, &clock);
    pthread_condattr_destroy(&clock);
    return result ? 4 : 0;
}

static void destroy(void) {
    pthread_cond_destroy(cond);
    pthread_mutex_destroy(mutex);
}

static int timed_condition(int clock, long milliseconds) {
    struct timespec start, end, until = deadline(clock, milliseconds);
    clock_gettime(1, &start);
    int result = pthread_cond_timedwait(cond, mutex, &until);
    clock_gettime(1, &end);
    if (result != 110) return 10; /* ETIMEDOUT */
    if (pthread_mutex_trylock(mutex) != 16) return 11; /* EBUSY: reacquired */
    long long elapsed = nanoseconds(end) - nanoseconds(start);
    return elapsed >= (milliseconds - 5) * 1000000 && elapsed < 2000000000 ? 0 : 12;
}

int probe_cond_fork_clock(void) {
    int result = initialize();
    if (result) return result;
    if (pthread_mutex_lock(mutex)) return 5;
    /* Populate this thread's native event/timer before transferring the guest. */
    result = timed_condition(1, 25);
    if (pthread_mutex_unlock(mutex) || result) return result ? result : 6;
    int child = fork();
    if (child < 0) return 7;
    if (!child) {
        if (pthread_mutex_lock(mutex)) _exit(20);
        result = timed_condition(1, 35);
        if (pthread_mutex_unlock(mutex) || result) _exit(result ? result : 21);
        /* A later default initialization must remove the inherited clock. */
        if (pthread_cond_destroy(cond) || pthread_cond_init(cond, 0)) _exit(22);
        if (pthread_mutex_lock(mutex)) _exit(23);
        result = timed_condition(0, 25);
        if (pthread_mutex_unlock(mutex) || result) _exit(result ? result : 24);
        destroy();
        _exit(0);
    }
    int status;
    if (waitpid(child, &status, 0) != child) return 8;
    destroy();
    return status ? 100 + ((status >> 8) & 255) : 0;
}

static void signal_handler(int number) {
    if (number == 10 && !pthread_mutex_trylock(mutex)) {
        handler_unlocked = pthread_mutex_unlock(mutex) == 0;
    }
    __atomic_store_n(&handled, 1, __ATOMIC_RELEASE);
}

static void *signal_waiter(void *unused) {
    (void)unused;
    if (pthread_mutex_lock(mutex)) return (void *)1;
    struct timespec until = deadline(1, 4000);
    __atomic_store_n(&ready, 1, __ATOMIC_RELEASE);
    int result = 0;
    while (!done && !result) result = pthread_cond_timedwait(cond, mutex, &until);
    if (pthread_mutex_unlock(mutex)) return (void *)2;
    return (void *)(word)result;
}

int probe_cond_signal(void) {
    int result = initialize();
    if (result) return result;
    ready = done = handled = handler_unlocked = 0;
    void (*previous)(int) = signal(10, signal_handler);
    word thread;
    if (pthread_create(&thread, 0, signal_waiter, 0)) return 5;
    while (!__atomic_load_n(&ready, __ATOMIC_ACQUIRE)) sched_yield();
    usleep(30000);
    result = pthread_kill(thread, 10);
    for (int attempt = 0; attempt < 1000 && !__atomic_load_n(&handled, __ATOMIC_ACQUIRE); ++attempt)
        usleep(1000);
    if (pthread_mutex_lock(mutex)) return 6;
    done = 1;
    pthread_cond_signal(cond);
    pthread_mutex_unlock(mutex);
    void *value;
    if (pthread_join(thread, &value)) return 7;
    signal(10, previous);
    destroy();
    if (result || value) return 8;
    return handled && handler_unlocked ? 0 : 9;
}

static void *mutex_waiter(void *unused) {
    (void)unused;
    struct timespec until = deadline(0, 2000);
    __atomic_store_n(&ready, 1, __ATOMIC_RELEASE);
    int result = pthread_mutex_timedlock(mutex, &until);
    if (!result) result = pthread_mutex_unlock(mutex);
    return (void *)(word)result;
}

int probe_timed_mutex_handoff(void) {
    int result = initialize();
    if (result) return result;
    ready = 0;
    if (pthread_mutex_lock(mutex)) return 5;
    word thread;
    if (pthread_create(&thread, 0, mutex_waiter, 0)) return 6;
    while (!__atomic_load_n(&ready, __ATOMIC_ACQUIRE)) sched_yield();
    usleep(25000);
    if (pthread_mutex_unlock(mutex)) return 7;
    void *value;
    if (pthread_join(thread, &value)) return 8;
    destroy();
    return value ? 9 : 0;
}
