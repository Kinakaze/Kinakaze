#ifndef KINAKAZE_STAT_H
#define KINAKAZE_STAT_H

#include <kinakaze/types.h>

/*
 * The Linux x86_64 struct stat: 144 bytes, including three reserved words at
 * the end. Field order mirrors kinakaze_vfs::fs::Stat and must not change.
 */
struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    unsigned int __pad0;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    time_t st_atime;
    long long st_atime_nsec;
    time_t st_mtime;
    long long st_mtime_nsec;
    time_t st_ctime;
    long long st_ctime_nsec;
    long long __unused[3];
};

#define S_IFMT 0170000
#define S_IFSOCK 0140000
#define S_IFLNK 0120000
#define S_IFREG 0100000
#define S_IFBLK 0060000
#define S_IFDIR 0040000
#define S_IFCHR 0020000
#define S_IFIFO 0010000

#define S_ISREG(mode) (((mode) & S_IFMT) == S_IFREG)
#define S_ISDIR(mode) (((mode) & S_IFMT) == S_IFDIR)
#define S_ISCHR(mode) (((mode) & S_IFMT) == S_IFCHR)
#define S_ISBLK(mode) (((mode) & S_IFMT) == S_IFBLK)
#define S_ISFIFO(mode) (((mode) & S_IFMT) == S_IFIFO)
#define S_ISLNK(mode) (((mode) & S_IFMT) == S_IFLNK)
#define S_ISSOCK(mode) (((mode) & S_IFMT) == S_IFSOCK)

#ifdef __cplusplus
extern "C" {
#endif

int stat(const char *path, struct stat *out);
int lstat(const char *path, struct stat *out);
int fstat(int fd, struct stat *out);
int mkdir(const char *path, mode_t mode);

#ifdef __cplusplus
}
#endif

#endif
