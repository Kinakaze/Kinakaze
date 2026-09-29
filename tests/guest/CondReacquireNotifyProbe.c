#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>

static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t condition = PTHREAD_COND_INITIALIZER;
static atomic_int entering;
static int finished;
static int timed;

static void *waiter(void *unused) {
    (void)unused;
    assert(pthread_mutex_lock(&mutex) == 0);
    struct timespec deadline;
    assert(clock_gettime(timed == 2 ? CLOCK_MONOTONIC : CLOCK_REALTIME, &deadline) == 0);
    deadline.tv_sec += 3;
    atomic_store(&entering, 1);
    while (!finished) {
        int result = timed == 2 ? pthread_cond_clockwait(&condition, &mutex, CLOCK_MONOTONIC, &deadline)
                   : timed == 1 ? pthread_cond_timedwait(&condition, &mutex, &deadline)
                                : pthread_cond_wait(&condition, &mutex);
        assert(result == 0);
    }
    assert(pthread_mutex_unlock(&mutex) == 0);
    return NULL;
}

int main(void) {
    for (timed = 0; timed < 3; ++timed) {
        atomic_store(&entering, 0);
        finished = 0;
        pthread_t thread;
        assert(pthread_create(&thread, NULL, waiter, NULL) == 0);
        while (!atomic_load(&entering)) usleep(1000);
        assert(pthread_mutex_lock(&mutex) == 0);
        usleep(250000);
        finished = 1;
        assert(pthread_cond_broadcast(&condition) == 0);
        assert(pthread_mutex_unlock(&mutex) == 0);
        assert(pthread_join(thread, NULL) == 0);
    }
    assert(pthread_mutex_lock(&mutex) == 0);
    struct timespec deadline;
    assert(clock_gettime(CLOCK_MONOTONIC, &deadline) == 0);
    deadline.tv_nsec += 20000000;
    if (deadline.tv_nsec >= 1000000000) {
        deadline.tv_nsec -= 1000000000;
        ++deadline.tv_sec;
    }
    int result;
    do {
        result = pthread_cond_clockwait(&condition, &mutex, CLOCK_MONOTONIC, &deadline);
    } while (result == 0);
    assert(result == ETIMEDOUT);
    assert(pthread_mutex_trylock(&mutex) == EBUSY);
    assert(pthread_mutex_unlock(&mutex) == 0);
    puts("COND_REACQUIRE_NOTIFY_OK");
}
