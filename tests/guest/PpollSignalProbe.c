#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t delivered;
static struct timespec delivered_at;
static void handled(int number) {
    delivered = number;
    clock_gettime(CLOCK_MONOTONIC, &delivered_at);
}

static int wait_poll(int raw, struct pollfd *fds, nfds_t count,
                     struct timespec *timeout, const sigset_t *mask) {
    return raw ? syscall(SYS_ppoll, fds, count, timeout, mask, 8)
               : ppoll(fds, count, timeout, mask);
}

static void check_mask(void) {
    sigset_t current;
    assert(sigprocmask(SIG_SETMASK, NULL, &current) == 0);
    assert(sigismember(&current, SIGUSR1) == 1);
}

static void interrupt_case(int raw, int socket_wait, int delayed, int restart) {
    struct sigaction action = {.sa_handler = handled, .sa_flags = restart ? SA_RESTART : 0};
    sigemptyset(&action.sa_mask);
    assert(sigaction(SIGUSR1, &action, NULL) == 0);
    sigset_t blocked, empty;
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGUSR1);
    sigemptyset(&empty);
    assert(sigprocmask(SIG_SETMASK, &blocked, NULL) == 0);
    delivered = 0;
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    struct pollfd fd = {.fd = pair[0], .events = POLLIN};
    pid_t sender = 0;
    if (delayed) {
        pid_t parent = getpid();
        sender = fork();
        assert(sender >= 0);
        if (!sender) {
            usleep(20000);
            assert(kill(parent, SIGUSR1) == 0);
            _exit(0);
        }
    } else {
        assert(raise(SIGUSR1) == 0);
    }
    struct timespec timeout = {.tv_nsec = 200000000};
    errno = 0;
    int result = wait_poll(raw, socket_wait ? &fd : NULL, socket_wait, &timeout, &empty);
    int error = errno;
    if (result != -1 || error != EINTR || delivered != SIGUSR1)
        fprintf(stderr, "ppoll raw=%d socket=%d delayed=%d restart=%d result=%d errno=%d signal=%d\n",
                raw, socket_wait, delayed, restart, result, error, delivered);
    assert(result == -1 && error == EINTR && delivered == SIGUSR1);
    check_mask();
    if (raw) {
        assert(timeout.tv_sec == 0 && timeout.tv_nsec < 200000000);
    } else {
        assert(timeout.tv_sec == 0 && timeout.tv_nsec == 200000000);
    }
    if (sender) {
        int status;
        assert(waitpid(sender, &status, 0) == sender);
        assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    }
    close(pair[0]);
    close(pair[1]);
}

static void boundaries(int raw) {
    sigset_t empty;
    sigemptyset(&empty);
    struct timespec invalid[] = {{-1, 0}, {0, -1}, {0, 1000000000}};
    for (unsigned i = 0; i < sizeof invalid / sizeof invalid[0]; ++i) {
        errno = 0;
        assert(wait_poll(raw, NULL, 0, &invalid[i], &empty) == -1 && errno == EINVAL);
        check_mask();
    }
    struct timespec zero = {0, 0};
    assert(wait_poll(raw, NULL, 1, &zero, &empty) == -1 && errno == EFAULT);
    check_mask();
    delivered = 0;
    assert(raise(SIGUSR1) == 0);
    assert(wait_poll(raw, NULL, 0, &zero, &empty) == -1 && errno == EINTR);
    assert(delivered == SIGUSR1);
    check_mask();
    assert(wait_poll(raw, NULL, 0, &zero, &empty) == 0);
    check_mask();
    assert(wait_poll(raw, NULL, 0, &zero, NULL) == 0);
    check_mask();
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    assert(write(pair[1], "x", 1) == 1);
    struct pollfd fd = {.fd = pair[0], .events = POLLIN};
    struct timespec huge = {LLONG_MAX, 999999999};
    delivered = 0;
    assert(raise(SIGUSR1) == 0);
    assert(wait_poll(raw, &fd, 1, &huge, &empty) == 1 && (fd.revents & POLLIN));
    assert(delivered == 0);
    check_mask();
    assert(wait_poll(raw, NULL, 0, &zero, &empty) == -1 && errno == EINTR);
    assert(delivered == SIGUSR1);
    close(pair[0]);
    close(pair[1]);
    if (raw) {
        assert(syscall(SYS_ppoll, NULL, 0, &zero, &empty, 128) == -1 && errno == EINVAL);
        assert(syscall(SYS_ppoll, NULL, 0, &zero, NULL, 128) == 0);
        struct timespec *readonly = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        assert(readonly != MAP_FAILED);
        *readonly = (struct timespec){.tv_nsec = 1000000};
        assert(mprotect(readonly, 4096, PROT_READ) == 0);
        assert(syscall(SYS_ppoll, NULL, 0, readonly, NULL, 8) == 0);
        assert(readonly->tv_nsec == 1000000);
        assert(munmap(readonly, 4096) == 0);
    }
}

static void temporary_block(int raw) {
    sigset_t empty, blocked;
    sigemptyset(&empty);
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGUSR1);
    assert(sigprocmask(SIG_SETMASK, &empty, NULL) == 0);
    delivered = 0;
    pid_t parent = getpid(), sender = fork();
    assert(sender >= 0);
    if (!sender) {
        usleep(10000);
        assert(kill(parent, SIGUSR1) == 0);
        _exit(0);
    }
    struct timespec begin, timeout = {.tv_nsec = 50000000};
    clock_gettime(CLOCK_MONOTONIC, &begin);
    assert(wait_poll(raw, NULL, 0, &timeout, &blocked) == 0);
    sigset_t current;
    assert(sigprocmask(SIG_SETMASK, NULL, &current) == 0);
    assert(sigismember(&current, SIGUSR1) == 0);
    // The saved unblocked mask is restored on return. Delivery may occur at
    // that boundary or the next, but never during the temporarily blocked wait.
    assert(sigprocmask(SIG_SETMASK, &empty, NULL) == 0);
    assert(delivered == SIGUSR1);
    double elapsed = delivered_at.tv_sec - begin.tv_sec +
        (delivered_at.tv_nsec - begin.tv_nsec) / 1e9;
    assert(elapsed >= .05);
    int status;
    assert(waitpid(sender, &status, 0) == sender && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

int main(void) {
    for (int raw = 0; raw <= 1; ++raw) {
        for (int socket_wait = 0; socket_wait <= 1; ++socket_wait)
            for (int delayed = 0; delayed <= 1; ++delayed)
                for (int restart = 0; restart <= 1; ++restart)
                    interrupt_case(raw, socket_wait, delayed, restart);
        boundaries(raw);
        temporary_block(raw);
    }
    puts("PPOLL_SIGNAL_OK");
}
