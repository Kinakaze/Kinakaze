#ifndef KINAKAZE_UNISTD_H
#define KINAKAZE_UNISTD_H

#include <kinakaze/types.h>

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

#ifdef __cplusplus
extern "C" {
#endif

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

#define F_OK 0
#define X_OK 1
#define W_OK 2
#define R_OK 4

ssize_t read(int fd, void *buffer, size_t count);
ssize_t write(int fd, const void *buffer, size_t count);
int close(int fd);
pid_t fork(void);
void _exit(int status) __attribute__((noreturn));

off_t lseek(int fd, off_t offset, int whence);
int unlink(const char *path);
int rmdir(const char *path);
int chdir(const char *path);
int access(const char *path, int mode);
int rename(const char *from, const char *to);
int ftruncate(int fd, off_t length);
char *getcwd(char *buffer, size_t size);
int isatty(int fd);
char *ttyname(int fd);
int ttyname_r(int fd, char *buf, size_t buflen);
pid_t tcgetpgrp(int fd);
int tcsetpgrp(int fd, pid_t pgrp);
pid_t getpid(void);
pid_t getppid(void);
unsigned int sleep(unsigned int seconds);
int usleep(unsigned int microseconds);

#ifdef __cplusplus
}
#endif

#endif
