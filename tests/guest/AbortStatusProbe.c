#define _GNU_SOURCE
#include <assert.h>
#include <signal.h>
#include <stdlib.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static int handler_fd;
static void handled(int signal) { assert(signal == SIGABRT); (void)write(handler_fd, "a", 1); }
extern void __chk_fail(void);

int main(void) {
    for (int mode = 0; mode != 5; ++mode) {
        int pipefd[2];
        assert(pipe(pipefd) == 0);
        pid_t child = fork();
        assert(child >= 0);
        if (!child) {
            close(pipefd[0]);
            handler_fd = pipefd[1];
            if (mode == 1) signal(SIGABRT, SIG_IGN);
            if (mode == 2) {
                signal(SIGABRT, handled);
                sigset_t blocked;
                sigemptyset(&blocked); sigaddset(&blocked, SIGABRT);
                sigprocmask(SIG_BLOCK, &blocked, NULL);
            }
            if (mode == 3) assert(!"intentional guest assertion");
            if (mode == 4) __chk_fail();
            abort();
        }
        close(pipefd[1]);
        int status;
        assert(waitpid(child, &status, 0) == child);
        assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
        char message[2];
        int length = read(pipefd[0], message, sizeof message);
        assert(length == (mode == 2 ? 1 : 0));
        close(pipefd[0]);
    }
    puts("ABORT_SIGNAL_STATUS_OK");
}
