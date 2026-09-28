#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <grp.h>
#include <gshadow.h>
#include <pwd.h>
#include <shadow.h>
#include <stdio.h>

#define CHECK_RECORD(function, text) do { \
    FILE *file = tmpfile(); \
    assert(file && fputs(text, file) >= 0); \
    rewind(file); \
    assert(function(file) != NULL && !feof(file)); \
    errno = 0; \
    assert(function(file) == NULL && errno == ENOENT && feof(file)); \
    assert(fclose(file) == 0); \
} while (0)

int main(void) {
    CHECK_RECORD(fgetpwent, "root:x:0:0:root:/root:/bin/sh\n");
    CHECK_RECORD(fgetgrent, "root:x:0:\n");
    CHECK_RECORD(fgetspent, "root:!:19000:0:99999:7:::\n");
    CHECK_RECORD(fgetsgent, "root:!::\n");
    puts("ACCOUNT_STREAM_EOF_OK");
}
