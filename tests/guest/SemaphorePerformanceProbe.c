#define _GNU_SOURCE
#include <assert.h>
#include <semaphore.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    const unsigned iterations = 1000000;
    sem_t sem;
    assert(sem_init(&sem, 0, 0) == 0);
    struct timespec start, end;
    assert(clock_gettime(CLOCK_MONOTONIC, &start) == 0);
    for (unsigned i = 0; i != iterations; ++i) {
        assert(sem_post(&sem) == 0 && sem_wait(&sem) == 0);
    }
    assert(clock_gettime(CLOCK_MONOTONIC, &end) == 0);
    double elapsed_ms = (end.tv_sec - start.tv_sec) * 1000.0
        + (end.tv_nsec - start.tv_nsec) / 1000000.0;
    assert(sem_destroy(&sem) == 0);
    printf("SEMAPHORE_PERF {\"iterations\":%u,\"elapsed_ms\":%.6f}\n", iterations, elapsed_ms);
}
