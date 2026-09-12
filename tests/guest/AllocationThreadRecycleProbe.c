/* Sequential short-lived workers must return reusable scratch storage while
 * preserving the live result transferred through pthread_join. */
typedef unsigned long size_t;
typedef unsigned long pthread_t;
extern void *malloc(size_t);
extern void free(void *);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
struct timespec { long sec, nsec; };
extern int clock_gettime(int, struct timespec *);
struct mallinfo2 { size_t arena,ordblks,smblks,hblks,hblkhd,usmblks,fsmblks,uordblks,fordblks,keepcost; };
extern struct mallinfo2 mallinfo2(void);
static double now(void) {
    struct timespec t; clock_gettime(1, &t);
    return t.sec * 1000.0 + t.nsec / 1000000.0;
}
struct job { unsigned seed; int error; };
static void *run(void *argument) {
    struct job *job = argument;
    unsigned char *blocks[96] = {0};
    unsigned char *result = malloc(73);
    if (!result) { job->error = 1; return 0; }
    for (unsigned i = 0; i < 73; ++i) result[i] = (unsigned char)(job->seed + i);
    for (unsigned i = 0; i < 96; ++i) {
        size_t size = i < 64 ? 4096 : 16384;
        blocks[i] = malloc(size);
        if (!blocks[i]) { job->error = 2; break; }
        for (size_t offset = 0; offset < size; offset += 4096)
            blocks[i][offset] = (unsigned char)(job->seed + i + offset / 4096);
        blocks[i][size - 1] = 0xa7;
    }
    for (unsigned i = 96; i-- != 0;) {
        if (!blocks[i]) continue;
        size_t size = i < 64 ? 4096 : 16384;
        for (size_t offset = 0; offset < size; offset += 4096)
            if (blocks[i][offset] != (unsigned char)(job->seed + i + offset / 4096)) job->error = 3;
        if (blocks[i][size - 1] != 0xa7) job->error = 4;
        free(blocks[i]);
    }
    return result;
}
int probe(double *milliseconds, size_t *stats) {
    struct mallinfo2 before = mallinfo2();
    for (unsigned round = 0; round < 4; ++round) {
        double start = now();
        for (unsigned i = 0; i < 24; ++i) {
            struct job job = {round * 24 + i, 0};
            pthread_t thread; void *returned = 0;
            if (pthread_create(&thread, 0, run, &job)) return 5;
            if (pthread_join(thread, &returned)) return 6;
            if (job.error || !returned) { free(returned); return 7; }
            unsigned char *result = returned;
            for (unsigned j = 0; j < 73; ++j)
                if (result[j] != (unsigned char)(job.seed + j)) { free(result); return 8; }
            free(result);
        }
        milliseconds[round] = now() - start;
        struct mallinfo2 after = mallinfo2();
        stats[round * 3] = after.arena - before.arena;
        stats[round * 3 + 1] = after.uordblks - before.uordblks;
        stats[round * 3 + 2] = after.fordblks - before.fordblks;
    }
    return 0;
}
