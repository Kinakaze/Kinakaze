#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <semaphore.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void child_ok(pid_t child) {
    int status;
    pid_t result;
    do { result = waitpid(child, &status, 0); } while (result < 0 && errno == EINTR);
    assert(result == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
static void read_one(int fd, char *value) {
    ssize_t count;
    do { count = read(fd, value, 1); } while (count < 0 && errno == EINTR);
    if (count != 1) fprintf(stderr, "read_one: count=%ld errno=%d\n", (long)count, errno);
    assert(count == 1);
}
static void byte(int fd) {
    struct pollfd event = {.fd = fd, .events = POLLIN};
    char value;
    int result;
    do { result = poll(&event, 1, 2000); } while (result < 0 && errno == EINTR);
    assert(result == 1);
    read_one(fd, &value);
}
static void take(sem_t *sem) {
    struct timespec deadline;
    assert(clock_gettime(CLOCK_REALTIME, &deadline) == 0);
    deadline.tv_sec += 2;
    assert(sem_timedwait(sem, &deadline) == 0);
}
int main(int argc, char **argv) {
    if (argc == 4) {
        sem_t *sem = sem_open(argv[2], 0);
        assert(sem != SEM_FAILED);
        int notify = atoi(argv[3]);
        assert(write(notify, "r", 1) == 1);
        take(sem);
        assert(sem_close(sem) == 0 && write(notify, "w", 1) == 1);
        return 0;
    }
    char name[128], backing[160];
    snprintf(name, sizeof name, "/kinakaze-sem-%ld", (long)getpid());
    snprintf(backing, sizeof backing, "/dev/shm/sem.%s", name + 1);
    sem_unlink(name);
    assert(sem_open(name, 0) == SEM_FAILED && errno == ENOENT);
    assert(sem_open("/invalid/name", O_CREAT, 0600, 0) == SEM_FAILED && errno == EINVAL);
    assert(sem_open("/", 0) == SEM_FAILED && errno == EINVAL);
    char long_name[260];
    memset(long_name, 'a', sizeof long_name - 1);
    long_name[sizeof long_name - 1] = 0;
    assert(sem_open(long_name, O_CREAT, 0600, 0) == SEM_FAILED && errno == ENAMETOOLONG);
    assert(sem_open(name, O_CREAT | O_EXCL, 0600, UINT_MAX) == SEM_FAILED && errno == EINVAL);

    mode_t old_mask = umask(0027);
    sem_t *first = sem_open(name, O_CREAT | O_EXCL, 0666, 0);
    umask(old_mask);
    assert(first != SEM_FAILED);
    struct stat status;
    assert(stat(backing, &status) == 0 && (status.st_mode & 0777) == 0640);
    assert(sem_open(name, O_CREAT | O_EXCL, 0600, 1) == SEM_FAILED && errno == EEXIST);
    sem_t *again = sem_open(name, O_CREAT, 0000, UINT_MAX);
    assert(again == first);
    assert(sem_trywait(first) == -1 && errno == EAGAIN);
    assert(sem_close(again) == 0);

    // Unlink only removes the name. New opens get a different object while
    // existing references and a fork child keep the old semaphore alive.
    assert(sem_unlink(name) == 0);
    assert(sem_open(name, 0) == SEM_FAILED && errno == ENOENT);
    sem_t *replacement = sem_open(name, O_CREAT | O_EXCL, 0600, 3);
    assert(replacement != SEM_FAILED && replacement != first);
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
        assert(sem_close(replacement) == 0);
        assert(sem_post(first) == 0 && sem_close(first) == 0);
        _exit(0);
    }
    take(first);
    child_ok(child);
    int value;
    assert(sem_getvalue(replacement, &value) == 0 && value == 3);
    assert(sem_close(first) == 0);
    assert(sem_close(first) == -1 && errno == EINVAL);
    for (int i = 0; i != 3; ++i) take(replacement);

    // A fresh exec reconnects by name to the same futex-backed mapping.
    int notify[2];
    assert(pipe(notify) == 0);
    child = fork();
    assert(child >= 0);
    if (!child) {
        close(notify[0]);
        char descriptor[32];
        snprintf(descriptor, sizeof descriptor, "%d", notify[1]);
        execl(argv[0], argv[0], "wait", name, descriptor, (char *)NULL);
        _exit(90);
    }
    close(notify[1]);
    byte(notify[0]);
    usleep(30000);
    assert(sem_post(replacement) == 0);
    byte(notify[0]);
    child_ok(child);
    close(notify[0]);
    assert(sem_close(replacement) == 0 && sem_unlink(name) == 0);

    // Racing creators publish precisely one initialized inode. The name keeps
    // its token after every creator has closed it and exited.
    int gate[2], reports[2];
    assert(pipe(gate) == 0 && pipe(reports) == 0);
    pid_t children[4];
    for (int i = 0; i != 4; ++i) {
        children[i] = fork();
        assert(children[i] >= 0);
        if (!children[i]) {
            close(gate[1]);
            char token;
            read_one(gate[0], &token);
            sem_t *sem = sem_open(name, O_CREAT | O_EXCL, 0600, 7);
            char won = sem != SEM_FAILED;
            if (won) assert(sem_close(sem) == 0);
            else assert(errno == EEXIST);
            assert(write(reports[1], &won, 1) == 1);
            _exit(0);
        }
    }
    close(gate[0]);
    close(reports[1]);
    assert(write(gate[1], "rrrr", 4) == 4);
    close(gate[1]);
    int winners = 0;
    for (int i = 0; i != 4; ++i) {
        char won;
        read_one(reports[0], &won);
        winners += won;
        child_ok(children[i]);
    }
    close(reports[0]);
    assert(winners == 1);
    sem_t *retained = sem_open(name, 0);
    assert(retained != SEM_FAILED && sem_getvalue(retained, &value) == 0 && value == 7);
    assert(sem_close(retained) == 0 && sem_unlink(name) == 0);
    assert(sem_unlink(name) == -1 && errno == ENOENT);
    puts("NAMED_SEMAPHORE_LIFETIME_OK");
}
