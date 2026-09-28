#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <pthread.h>
#include <semaphore.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void readable(int fd) {
    struct pollfd pollfd = {.fd = fd, .events = POLLIN};
    assert(poll(&pollfd, 1, 1000) == 1);
    char byte;
    assert(read(fd, &byte, 1) == 1);
}
static void signal_handler(int signal) { (void) signal; }
static void *interrupt_wait(void *argument) {
    usleep(30000);
    assert(pthread_kill(*(pthread_t *)argument, SIGUSR1) == 0);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc == 3) {
        int fd = atoi(argv[1]), notify = atoi(argv[2]);
        sem_t *sem = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        assert(sem != MAP_FAILED);
        assert(write(notify, "r", 1) == 1);
        assert(sem_wait(sem) == 0);
        assert(write(notify, "w", 1) == 1);
        assert(munmap(sem, 4096) == 0);
        return 0;
    }
    sem_t local;
    assert(sem_init(&local, 0, UINT_MAX) == -1 && errno == EINVAL);
    assert(sem_init(&local, 0, SEM_VALUE_MAX) == 0);
    assert(sem_post(&local) == -1 && errno == EOVERFLOW);
    assert(sem_destroy(&local) == 0 && sem_init(&local, 0, 0) == 0);
    assert(sem_trywait(&local) == -1 && errno == EAGAIN);
    struct timespec expired = {.tv_sec = -1};
    assert(sem_clockwait(&local, CLOCK_MONOTONIC, &expired) == -1 && errno == ETIMEDOUT);
    assert(sem_post(&local) == 0 && sem_timedwait(&local, &expired) == 0);
    struct sigaction action = {.sa_handler = signal_handler};
    assert(sigaction(SIGUSR1, &action, NULL) == 0);
    pthread_t self = pthread_self(), helper;
    assert(pthread_create(&helper, NULL, interrupt_wait, &self) == 0);
    assert(sem_wait(&local) == -1 && errno == EINTR);
    assert(pthread_join(helper, NULL) == 0 && sem_destroy(&local) == 0);
    puts("SEMAPHORE_PRIVATE_BOUNDARIES_OK");
    fflush(stdout);

    char filename[] = "/tmp/kinakaze-sem-XXXXXX";
    int fd = mkstemp(filename);
    assert(fd >= 0 && unlink(filename) == 0 && ftruncate(fd, 4096) == 0);
    sem_t *shared = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    sem_t *alias = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    assert(shared != MAP_FAILED && alias != MAP_FAILED && shared != alias);
    assert(sem_init(shared, 1, 0) == 0);
    for (int use_exec = 0; use_exec != 2; ++use_exec) {
        int pipes[2][2];
        pid_t children[2];
        for (int child = 0; child != 2; ++child) {
            assert(pipe(pipes[child]) == 0);
            children[child] = fork();
            assert(children[child] >= 0);
            if (!children[child]) {
                close(pipes[child][0]);
                if (use_exec) {
                    char descriptor[32], notify[32];
                    snprintf(descriptor, sizeof descriptor, "%d", fd);
                    snprintf(notify, sizeof notify, "%d", pipes[child][1]);
                    execl(argv[0], argv[0], descriptor, notify, (char *)NULL);
                    _exit(90);
                }
                assert(write(pipes[child][1], "r", 1) == 1 && sem_wait(shared) == 0);
                assert(write(pipes[child][1], "w", 1) == 1);
                _exit(0);
            }
            close(pipes[child][1]);
            readable(pipes[child][0]);
        }
        usleep(30000);
        assert(sem_post(alias) == 0 && sem_post(alias) == 0);
        for (int child = 0; child != 2; ++child) {
            readable(pipes[child][0]);
            int status;
            assert(waitpid(children[child], &status, 0) == children[child]);
            assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
            close(pipes[child][0]);
        }
    }
    assert(sem_trywait(shared) == -1 && errno == EAGAIN);
    assert(sem_destroy(shared) == 0);
    assert(munmap(shared, 4096) == 0 && munmap(alias, 4096) == 0);
    close(fd);
    puts("SEMAPHORE_SHARED_FORK_EXEC_ALIAS_OK");
}
