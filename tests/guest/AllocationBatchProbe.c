/* Exercise warm local reuse and concurrent central-bin refill/spill, including
 * actual payload validation. Timings exclude thread creation and warm-up. */
typedef unsigned long size_t;
typedef unsigned long pthread_t;
extern void *malloc(size_t);
extern void free(void *);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
extern int sched_yield(void);
struct timespec { long sec, nsec; };
extern int clock_gettime(int, struct timespec *);
static double now(void) {
    struct timespec t; clock_gettime(1, &t);
    return t.sec * 1000.0 + t.nsec / 1000000.0;
}
struct job {
    unsigned batch, rounds, id;
    int ready, start, done, error;
};
static int cycle(struct job *job) {
    unsigned char *blocks[512];
    for (unsigned i = 0; i < job->batch; ++i) {
        blocks[i] = malloc(256);
        if (!blocks[i]) {
            while (i) free(blocks[--i]);
            return 1;
        }
        blocks[i][0] = (unsigned char)i;
        blocks[i][1] = (unsigned char)(i >> 8);
        blocks[i][255] = (unsigned char)job->id;
    }
    int error = 0;
    for (unsigned i = job->batch; i-- != 0;) {
        error |= blocks[i][0] != (unsigned char)i;
        error |= blocks[i][1] != (unsigned char)(i >> 8);
        error |= blocks[i][255] != (unsigned char)job->id;
        free(blocks[i]);
    }
    return error;
}
static void *run(void *arg) {
    struct job *job = arg;
    job->error = cycle(job);
    __atomic_store_n(&job->ready, 1, __ATOMIC_RELEASE);
    while (!__atomic_load_n(&job->start, __ATOMIC_ACQUIRE)) sched_yield();
    for (unsigned i = 0; i < job->rounds && !job->error; ++i)
        job->error = cycle(job);
    __atomic_store_n(&job->done, 1, __ATOMIC_RELEASE);
    return 0;
}
int probe(unsigned count, unsigned batch, unsigned rounds, double *elapsed) {
    if (!count || count > 8 || !batch || batch > 512 || !rounds) return 2;
    struct job jobs[8] = {0}; pthread_t threads[8];
    unsigned created = 0;
    for (; created < count; ++created) {
        jobs[created].batch = batch; jobs[created].rounds = rounds;
        jobs[created].id = created + 37;
        if (pthread_create(&threads[created], 0, run, &jobs[created])) break;
    }
    for (unsigned i = 0; i < created; ++i)
        while (!__atomic_load_n(&jobs[i].ready, __ATOMIC_ACQUIRE)) sched_yield();
    double start = now();
    for (unsigned i = 0; i < created; ++i)
        __atomic_store_n(&jobs[i].start, 1, __ATOMIC_RELEASE);
    for (unsigned i = 0; i < created; ++i)
        while (!__atomic_load_n(&jobs[i].done, __ATOMIC_ACQUIRE)) sched_yield();
    *elapsed = now() - start;
    int error = created != count;
    for (unsigned i = 0; i < created; ++i) {
        error |= pthread_join(threads[i], 0) != 0;
        error |= jobs[i].error;
    }
    return error;
}
