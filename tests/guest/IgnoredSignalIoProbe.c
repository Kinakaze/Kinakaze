#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    struct sigaction action = {.sa_handler = SIG_DFL};
    assert(sigaction(SIGCHLD, &action, NULL) == 0);
    for (int round = 0; round != 24; ++round) {
        int descriptors[2];
        assert(pipe(descriptors) == 0);
        pid_t writer = fork();
        assert(writer >= 0);
        if (!writer) {
            close(descriptors[0]);
            usleep(80000);
            assert(write(descriptors[1], "x", 1) == 1);
            _exit(0);
        }
        pid_t exiting = fork();
        assert(exiting >= 0);
        if (!exiting) { usleep(10000); _exit(0); }
        close(descriptors[1]);
        char value;
        ssize_t count = read(descriptors[0], &value, 1);
        if (count != 1) fprintf(stderr, "ignored SIGCHLD read: count=%ld errno=%d round=%d\n", (long)count, errno, round);
        assert(count == 1 && value == 'x');
        close(descriptors[0]);
        int status;
        assert(waitpid(writer, &status, 0) == writer && WIFEXITED(status) && WEXITSTATUS(status) == 0);
        assert(waitpid(exiting, &status, 0) == exiting && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    }
    puts("IGNORED_SIGNAL_IO_OK");
}
