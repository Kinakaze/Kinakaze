#ifndef KINAKAZE_FCNTL_H
#define KINAKAZE_FCNTL_H

#include <kinakaze/types.h>

/* Linux x86_64 open flags, in octal as the kernel headers define them. */
#define O_RDONLY 00
#define O_WRONLY 01
#define O_RDWR 02
#define O_ACCMODE 03
#define O_CREAT 0100
#define O_EXCL 0200
#define O_NOCTTY 0400
#define O_TRUNC 01000
#define O_APPEND 02000
#define O_NONBLOCK 04000
#define O_DIRECTORY 0200000
#define O_NOFOLLOW 0400000
#define O_CLOEXEC 02000000

#define AT_FDCWD (-100)

#ifdef __cplusplus
extern "C" {
#endif

int open(const char *path, int flags, ...);
int openat(int dirfd, const char *path, int flags, ...);
int creat(const char *path, mode_t mode);

#ifdef __cplusplus
}
#endif

#endif
