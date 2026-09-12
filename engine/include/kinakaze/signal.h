#ifndef KINAKAZE_SIGNAL_H
#define KINAKAZE_SIGNAL_H

#include <kinakaze/types.h>

/* Linux x86_64 signal numbers. */
#define SIGHUP 1
#define SIGINT 2
#define SIGQUIT 3
#define SIGILL 4
#define SIGABRT 6
#define SIGFPE 8
#define SIGKILL 9
#define SIGUSR1 10
#define SIGSEGV 11
#define SIGUSR2 12
#define SIGPIPE 13
#define SIGALRM 14
#define SIGTERM 15
#define SIGCHLD 17
#define SIGCONT 18
#define SIGSTOP 19
#define SIGTSTP 20
#define SIGWINCH 28

#define SIG_DFL ((void (*)(int))0)
#define SIG_IGN ((void (*)(int))1)
#define SIG_ERR ((void (*)(int))-1)

#define SA_NOCLDSTOP 1
#define SA_SIGINFO 4
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000

#define SIG_BLOCK 0
#define SIG_UNBLOCK 1
#define SIG_SETMASK 2

typedef void (*sighandler_t)(int);

/* 128 bytes, matching the Linux x86_64 sigset_t. */
typedef struct {
    unsigned long long bits[16];
} sigset_t;

/* Field order matches glibc Linux x86_64 struct sigaction. */
struct sigaction {
    sighandler_t sa_handler;
    sigset_t sa_mask;
    int sa_flags;
    int _padding;
    void (*sa_restorer)(void);
};

#ifdef __cplusplus
extern "C" {
#endif

sighandler_t signal(int signal_number, sighandler_t handler);
int sigaction(int signal_number, const struct sigaction *action,
              struct sigaction *old_action);
int raise(int signal_number);
int kill(pid_t pid, int signal_number);

int sigprocmask(int how, const sigset_t *set, sigset_t *old_set);
int sigpending(sigset_t *set);
int sigemptyset(sigset_t *set);
int sigfillset(sigset_t *set);
int sigaddset(sigset_t *set, int signal_number);
int sigdelset(sigset_t *set, int signal_number);
int sigismember(const sigset_t *set, int signal_number);

#ifdef __cplusplus
}
#endif

#endif
