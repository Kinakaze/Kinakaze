/* Real GNU cleanup frames ensure cond cancellation reacquires the guest mutex. */
typedef unsigned long word;
typedef struct __attribute__((aligned(16))) {
    word registers[8]; int mask_saved; int pad; void *private[4];
} frame;
struct timespec { long seconds, nanoseconds; };
extern int __sigsetjmp(void *, int) __attribute__((returns_twice));
extern void __pthread_register_cancel(frame *);
extern void __pthread_unwind_next(frame *) __attribute__((noreturn));
extern int pthread_create(word *, const void *, void *(*)(void *), void *);
extern int pthread_join(word, void **);
extern int pthread_cancel(word);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_trylock(void *);
extern int pthread_mutex_unlock(void *);
extern int pthread_cond_init(void *, const void *);
extern int pthread_cond_destroy(void *);
extern int pthread_cond_wait(void *, void *);
extern int pthread_cond_timedwait(void *, void *, const struct timespec *);
extern int clock_gettime(int, struct timespec *);
extern int sched_yield(void);
extern int usleep(unsigned int);
static word mutex[5], cond[6];
static int ready, cleaned, held;

static void *waiter(void *mode) {
    frame cleanup;
    pthread_mutex_lock(mutex);
    if (__sigsetjmp(&cleanup, 0)) {
        held = pthread_mutex_trylock(mutex) == 16; /* EBUSY */
        cleaned = pthread_mutex_unlock(mutex) == 0;
        __pthread_unwind_next(&cleanup);
    }
    __pthread_register_cancel(&cleanup);
    struct timespec deadline;
    clock_gettime(0, &deadline);
    deadline.seconds += 3600;
    __atomic_store_n(&ready, 1, __ATOMIC_RELEASE);
    for (;;) {
        if (mode) pthread_cond_timedwait(cond, mutex, &deadline);
        else pthread_cond_wait(cond, mutex);
    }
}

int probe_cond_cancel(int timed) {
    ready = cleaned = held = 0;
    if (pthread_mutex_init(mutex, 0) || pthread_cond_init(cond, 0)) return 1;
    word thread;
    if (pthread_create(&thread, 0, waiter, (void *)(word)timed)) return 2;
    while (!__atomic_load_n(&ready, __ATOMIC_ACQUIRE)) sched_yield();
    usleep(30000); /* Exercise cancellation after the native wait has begun. */
    if (pthread_cancel(thread)) return 3;
    void *result;
    if (pthread_join(thread, &result)) return 4;
    if (result != (void *)~(word)0 || !cleaned || !held) return 5;
    if (pthread_mutex_trylock(mutex)) return 6;
    pthread_mutex_unlock(mutex);
    pthread_cond_destroy(cond);
    pthread_mutex_destroy(mutex);
    return 0;
}
