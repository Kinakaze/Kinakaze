#ifndef KINAKAZE_DIRENT_H
#define KINAKAZE_DIRENT_H

#include <kinakaze/types.h>

#define DT_UNKNOWN 0
#define DT_DIR 4
#define DT_REG 8

/* DIR is opaque: the snapshot lives inside libc.dll. */
typedef struct Dir DIR;

/* Field order and the 256-byte name match the Linux x86_64 struct dirent. */
struct dirent {
    ino_t d_ino;
    off_t d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[256];
};

#ifdef __cplusplus
extern "C" {
#endif

DIR *opendir(const char *path);
struct dirent *readdir(DIR *directory);
int closedir(DIR *directory);
void rewinddir(DIR *directory);
long telldir(DIR *directory);
void seekdir(DIR *directory, long position);

#ifdef __cplusplus
}
#endif

#endif
