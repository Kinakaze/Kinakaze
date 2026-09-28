#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <grp.h>
#include <gshadow.h>
#include <pwd.h>
#include <shadow.h>
#include <stdio.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include <time.h>

#define CHECK_RECORD(function, text) do { \
    FILE *file = tmpfile(); \
    assert(file && fputs(text, file) >= 0); \
    rewind(file); \
    assert(function(file) != NULL && !feof(file)); \
    errno = 0; \
    assert(function(file) == NULL && errno == ENOENT && feof(file)); \
    assert(fclose(file) == 0); \
} while (0)

enum { TEXT_LENGTH = 16384, MEMBERS = 1500, READERS = 4, RECORDS = 1000 };

static FILE *passwd_file(void) {
    FILE *file = tmpfile();
    assert(file);
    assert(fputs("long:x:1234:5678:", file) >= 0);
    for (int i = 0; i < TEXT_LENGTH; ++i) assert(fputc('a', file) == 'a');
    assert(fputs(":/home/long:/bin/sh\nnext:x:7:8::/:/bin/sh\n", file) >= 0);
    rewind(file);
    return file;
}

static FILE *group_file(void) {
    FILE *file = tmpfile();
    assert(file && fputs("large:x:5678:", file) >= 0);
    for (int i = 0; i < MEMBERS; ++i) assert(fprintf(file, "%su%d", i ? "," : "", i) > 0);
    assert(fputs("\nnext:x:8:\n", file) >= 0);
    rewind(file);
    return file;
}

static void check_user(const struct passwd *user) {
    assert(user && strcmp(user->pw_name, "long") == 0);
    assert(user->pw_uid == 1234 && user->pw_gid == 5678);
    assert(strlen(user->pw_gecos) == TEXT_LENGTH && user->pw_gecos[TEXT_LENGTH - 1] == 'a');
    assert(strcmp(user->pw_shell, "/bin/sh") == 0);
}

static void check_group(const struct group *group) {
    assert(group && strcmp(group->gr_name, "large") == 0 && group->gr_gid == 5678);
    for (int i = 0; i < MEMBERS; ++i) {
        char name[32];
        snprintf(name, sizeof name, "u%d", i);
        assert(group->gr_mem[i] && strcmp(group->gr_mem[i], name) == 0);
    }
    assert(group->gr_mem[MEMBERS] == NULL);
}

static void reentrant(void) {
    char *buffer = malloc(65537);
    assert(buffer);
    struct passwd pw, *user = &pw;
    FILE *file = passwd_file();
    for (size_t size = 0; size < TEXT_LENGTH; size = size ? size * 2 : 1) {
        errno = 0;
        assert(fgetpwent_r(file, &pw, buffer, size, &user) == ERANGE);
        assert(user == NULL && errno == ERANGE && ftell(file) == 0 && !ferror(file) && !feof(file));
    }
    assert(fgetpwent_r(file, &pw, buffer, 65536, &user) == 0 && user == &pw);
    check_user(user);
    assert(fgetpwent_r(file, &pw, buffer, 65536, &user) == 0 && !strcmp(user->pw_name, "next"));
    assert(fgetpwent_r(file, &pw, buffer, 65536, &user) == ENOENT && user == NULL && feof(file));
    assert(fclose(file) == 0);

    struct group gr, *group = &gr;
    file = group_file();
    /* The text fits here, but the member pointer table does not. */
    assert(fgetgrent_r(file, &gr, buffer + 1, 10000, &group) == ERANGE);
    assert(group == NULL && ftell(file) == 0 && !ferror(file));
    assert(fgetgrent_r(file, &gr, buffer + 1, 65536, &group) == 0 && group == &gr);
    check_group(group);
    assert(fgetgrent_r(file, &gr, buffer, 65536, &group) == 0 && !strcmp(group->gr_name, "next"));
    assert(fclose(file) == 0);

    file = tmpfile();
    assert(file && fputs("shadow:", file) >= 0);
    for (int i = 0; i < TEXT_LENGTH; ++i) fputc('x', file);
    assert(fputs(":19000:0:99999:7:::\n", file) >= 0);
    rewind(file);
    struct spwd sp, *shadow = &sp;
    assert(fgetspent_r(file, &sp, buffer, 128, &shadow) == ERANGE);
    assert(!shadow && ftell(file) == 0);
    assert(fgetspent_r(file, &sp, buffer, 65536, &shadow) == 0 && shadow == &sp);
    assert(strlen(shadow->sp_pwdp) == TEXT_LENGTH && shadow->sp_lstchg == 19000);
    assert(fclose(file) == 0);
    free(buffer);
}

static void ownership(void) {
    FILE *users = passwd_file(), *groups = group_file();
    struct passwd *user = fgetpwent(users);
    struct group *group = fgetgrent(groups);
    check_user(user); check_group(group);
    assert(fclose(users) == 0 && fclose(groups) == 0);
    /* File reads have separate result storage from database lookups. */
    assert(getpwnam("root") && getgrnam("root"));
    check_user(user); check_group(group);
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
        check_user(user); check_group(group);
        users = passwd_file(); groups = group_file();
        check_user(fgetpwent(users)); check_group(fgetgrent(groups));
        assert(fclose(users) == 0 && fclose(groups) == 0);
        _exit(0);
    }
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    check_user(user); check_group(group);
}

static void boundaries(void) {
    FILE *file = tmpfile();
    assert(file && fputs(" # comment\ninvalid:x:wrong:0::/:/bin/sh\n\n", file) >= 0);
    assert(fputs("  bytes:x:42:43:\xff:/home/bytes:/bin/sh", file) >= 0);
    rewind(file);
    struct passwd *user = fgetpwent(file);
    assert(user && !strcmp(user->pw_name, "bytes") && user->pw_uid == 42);
    assert((unsigned char)user->pw_gecos[0] == 255 && !user->pw_gecos[1]);
    assert(fclose(file) == 0);

    int pipes[2];
    assert(pipe(pipes) == 0);
    const char text[] = "pipe:x:1:2::/:/bin/sh\n";
    assert(write(pipes[1], text, sizeof text - 1) == sizeof text - 1);
    assert(close(pipes[1]) == 0);
    file = fdopen(pipes[0], "r");
    assert(file);
    char buffer[256];
    struct passwd pw;
    assert(fgetpwent_r(file, &pw, buffer, 8, &user) == ESPIPE && !user && ferror(file));
    assert(fclose(file) == 0);

    assert(pipe(pipes) == 0);
    file = fdopen(pipes[1], "w");
    assert(file && close(pipes[0]) == 0);
    assert(fgetpwent_r(file, &pw, buffer, sizeof buffer, &user) == EBADF);
    assert(!user && ferror(file));
    assert(fclose(file) == 0);
}

struct reader { FILE *file; unsigned seen[RECORDS]; };
static void *read_entries(void *opaque) {
    struct reader *reader = opaque;
    struct passwd entry, *result;
    char buffer[256];
    int code;
    while (!(code = fgetpwent_r(reader->file, &entry, buffer, sizeof buffer, &result))) {
        assert(result == &entry && entry.pw_uid < RECORDS);
        ++reader->seen[entry.pw_uid];
    }
    assert(code == ENOENT && !result);
    return NULL;
}

static void concurrent(void) {
    FILE *file = tmpfile();
    assert(file);
    for (int i = 0; i < RECORDS; ++i) assert(fprintf(file, "user%d:x:%d:0::/:/bin/sh\n", i, i) > 0);
    rewind(file);
    struct reader readers[READERS] = {0};
    pthread_t threads[READERS];
    for (int i = 0; i < READERS; ++i) {
        readers[i].file = file;
        assert(pthread_create(&threads[i], NULL, read_entries, &readers[i]) == 0);
    }
    for (int i = 0; i < READERS; ++i) assert(pthread_join(threads[i], NULL) == 0);
    for (int i = 0; i < RECORDS; ++i) {
        unsigned count = 0;
        for (int j = 0; j < READERS; ++j) count += readers[j].seen[i];
        assert(count == 1);
    }
    assert(fclose(file) == 0);
}

static void benchmark(void) {
    for (int kind = 0; kind < 4; ++kind) {
        FILE *file = tmpfile();
        assert(file);
        const int count = 20000;
        for (int i = 0; i < count; ++i) {
            assert(fputs(kind < 2 ? "user:x:1234:5678:Example:/home/user:/bin/sh\n" :
                         "group:x:5678:first,second,third,fourth\n", file) >= 0);
        }
        rewind(file);
        struct timespec start, end;
        assert(clock_gettime(CLOCK_MONOTONIC, &start) == 0);
        for (int i = 0; i < count; ++i) {
            char buffer[1024];
            struct passwd pw, *user;
            struct group gr, *group;
            switch (kind) {
            case 0: assert(fgetpwent_r(file, &pw, buffer, sizeof buffer, &user) == 0 && user->pw_uid == 1234); break;
            case 1: assert((user = fgetpwent(file)) && user->pw_uid == 1234); break;
            case 2: assert(fgetgrent_r(file, &gr, buffer, sizeof buffer, &group) == 0 && group->gr_gid == 5678); break;
            case 3: assert((group = fgetgrent(file)) && group->gr_gid == 5678); break;
            }
        }
        assert(clock_gettime(CLOCK_MONOTONIC, &end) == 0);
        printf("account-stream-%d %d %.3f\n", kind, count,
               (end.tv_sec - start.tv_sec) * 1000.0 + (end.tv_nsec - start.tv_nsec) / 1000000.0);
        assert(fclose(file) == 0);
    }
}

int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "benchmark")) {
        benchmark();
        return 0;
    }
    CHECK_RECORD(fgetpwent, "root:x:0:0:root:/root:/bin/sh\n");
    CHECK_RECORD(fgetgrent, "root:x:0:\n");
    CHECK_RECORD(fgetspent, "root:!:19000:0:99999:7:::\n");
    CHECK_RECORD(fgetsgent, "root:!::\n");
    reentrant();
    ownership();
    boundaries();
    concurrent();
    puts("ACCOUNT_STREAM_EOF_OK");
}
