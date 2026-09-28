#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static _Thread_local uint64_t initialized = UINT64_C(0x123456789abcdef0);
static _Alignas(64) _Thread_local unsigned char zeroed[73];

static void check_initial(void) {
    assert(initialized == UINT64_C(0x123456789abcdef0));
    assert((uintptr_t)zeroed % 64 == 0);
    for (unsigned i = 0; i < sizeof zeroed; ++i) assert(zeroed[i] == 0);
    assert(close(-1) == -1 && errno == EBADF);
    assert(initialized == UINT64_C(0x123456789abcdef0));
}

static void *thread(void *unused) {
    (void)unused;
    check_initial();
    initialized = 19;
    zeroed[72] = 20;
    errno = EACCES;
    return NULL;
}

int main(void) {
    check_initial();
    initialized = 42;
    zeroed[72] = 43;
    errno = ENOENT;
    pthread_t worker;
    assert(pthread_create(&worker, NULL, thread, NULL) == 0);
    assert(pthread_join(worker, NULL) == 0);
    assert(initialized == 42 && zeroed[72] == 43 && errno == ENOENT);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(initialized == 42 && zeroed[72] == 43);
        assert(close(-1) == -1 && errno == EBADF);
        initialized = 99;
        zeroed[72] = 100;
        _exit(23);
    }
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 23);
    assert(initialized == 42 && zeroed[72] == 43);
    puts("EXECUTABLE_TLS_OK");
}
