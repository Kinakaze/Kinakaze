#ifndef KINAKAZE_PTY_H
#define KINAKAZE_PTY_H

#include <kinakaze/termios.h>
#include <kinakaze/types.h>

#ifdef __cplusplus
extern "C" {
#endif

int openpty(int *amaster, int *aslave, char *name,
            const struct termios *termp, const struct winsize *winp);
pid_t forkpty(int *amaster, char *name,
              const struct termios *termp, const struct winsize *winp);
int login_tty(int fd);

#ifdef __cplusplus
}
#endif

#endif
