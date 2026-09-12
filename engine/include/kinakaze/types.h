#ifndef KINAKAZE_TYPES_H
#define KINAKAZE_TYPES_H

typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ssize_t;
typedef int pid_t;

/* Linux x86_64 widths. These appear in struct stat, so they are ABI. */
typedef unsigned long long dev_t;
typedef unsigned long long ino_t;
typedef unsigned int mode_t;
typedef unsigned long long nlink_t;
typedef unsigned int uid_t;
typedef unsigned int gid_t;
typedef long long off_t;
typedef long long blksize_t;
typedef long long blkcnt_t;

/*
 * time_t is shared with <time.h>. Both headers define it through this guard so
 * the two declarations can never drift apart.
 */
#ifndef KINAKAZE_TIME_T_DEFINED
#define KINAKAZE_TIME_T_DEFINED
typedef long time_t;
#endif

#endif
