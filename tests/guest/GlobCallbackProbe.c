#define _GNU_SOURCE
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fnmatch.h>
#include <glob.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>

/* No /virtual tree exists: matches must use the caller's filesystem. */
static const char *names[] = {"b", "a", "sub", ".hidden", "dangling", "star*"};
static int opens, closes, stats, lstats, read_error, error_calls;
struct stream { size_t next; struct dirent entry; };

static void *open_dir(const char *path) {
    ++opens;
    if (strcmp(path, "/virtual") && strcmp(path, "/virtual/")) {
        errno = !strcmp(path, "/denied") ? EACCES : ENOENT;
        return NULL;
    }
    return calloc(1, sizeof(struct stream));
}
static struct dirent *read_dir(void *opaque) {
    struct stream *s = opaque;
    if (read_error) { errno = EIO; return NULL; }
    if (s->next == sizeof(names) / sizeof(*names)) { errno = 0; return NULL; }
    strcpy(s->entry.d_name, names[s->next++]);
    s->entry.d_type = DT_UNKNOWN;
    return &s->entry;
}
static void close_dir(void *opaque) { ++closes; free(opaque); }
static int metadata(const char *path, struct stat *st, int follow) {
    if (follow) ++stats; else ++lstats;
    memset(st, 0, sizeof(*st));
    if (!strcmp(path, "/virtual") || !strcmp(path, "/virtual/sub")) {
        st->st_mode = S_IFDIR | 0755;
        return 0;
    }
    for (size_t i = 0; i < sizeof(names) / sizeof(*names); ++i) {
        char expected[128];
        snprintf(expected, sizeof(expected), "/virtual/%s", names[i]);
        if (strcmp(path, expected)) continue;
        if (!strcmp(names[i], "dangling")) {
            if (follow) { errno = ENOENT; return -1; }
            st->st_mode = S_IFLNK | 0777;
        } else st->st_mode = S_IFREG | 0644;
        return 0;
    }
    errno = ENOENT;
    return -1;
}
static int stat_path(const char *path, struct stat *st) { return metadata(path, st, 1); }
static int lstat_path(const char *path, struct stat *st) { return metadata(path, st, 0); }
static int on_error(const char *path, int error) {
    assert(!strcmp(path, "/denied") || !strcmp(path, "/virtual"));
    assert(error == EACCES || error == EIO);
    ++error_calls;
    return 1;
}
static glob_t setup(void) {
    glob_t g = {0};
    g.gl_opendir = open_dir; g.gl_readdir = read_dir; g.gl_closedir = close_dir;
    g.gl_stat = stat_path; g.gl_lstat = lstat_path;
    return g;
}
static void expect(glob_t *g, size_t at, const char *name) {
    assert(at < g->gl_pathc);
    assert(!strcmp(g->gl_pathv[g->gl_offs + at], name));
    assert(g->gl_pathv[g->gl_offs + g->gl_pathc] == NULL);
}
int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "native")) {
        struct timespec begin, end;
        clock_gettime(CLOCK_MONOTONIC, &begin);
        for (int i = 0; i < 50; ++i) {
            glob_t native = {0};
            assert(glob("/etc/os-release", 0, NULL, &native) == 0);
            assert(native.gl_pathc == 1);
            expect(&native, 0, "/etc/os-release");
            globfree(&native);
        }
        clock_gettime(CLOCK_MONOTONIC, &end);
        printf("GLOB_LITERAL_50_MS=%.3f\n", (end.tv_sec-begin.tv_sec)*1000.0 + (end.tv_nsec-begin.tv_nsec)/1e6);
        puts("GLOB_NATIVE_OK");
        return 0;
    }
    glob_t g = setup();
    assert(FNM_PATHNAME == 1 && FNM_NOESCAPE == 2);
    assert(fnmatch("*", "a/b", FNM_PATHNAME) == FNM_NOMATCH);
    assert(fnmatch("a\\*", "a*", 0) == 0);
    assert(fnmatch("a\\*", "a*", FNM_NOESCAPE) == FNM_NOMATCH);
    assert(fnmatch("a\\*", "a\\tail", FNM_NOESCAPE) == 0);
    int flags = GLOB_ALTDIRFUNC;
    assert(glob("/virtual/a", flags, NULL, &g) == 0);
    expect(&g, 0, "/virtual/a");
    assert(opens == 0 && lstats == 1 && stats == 0);
    globfree(&g);
    assert(glob("/virtual/*", flags, NULL, &g) == 0);
    assert(g.gl_pathc == 5 && opens == 1 && closes == 1);
    expect(&g, 0, "/virtual/a"); expect(&g, 2, "/virtual/dangling");
    assert(g.gl_flags & GLOB_MAGCHAR);
    globfree(&g);
    assert(glob("/virtual/*", flags | GLOB_ONLYDIR | GLOB_MARK, NULL, &g) == 0);
    assert(g.gl_pathc == 1); expect(&g, 0, "/virtual/sub/"); globfree(&g);
    assert(glob("/virtual/*", flags | GLOB_PERIOD, NULL, &g) == 0);
    assert(g.gl_pathc == 6); expect(&g, 0, "/virtual/.hidden"); globfree(&g);
    assert(glob("/virtual/star\\*", flags, NULL, &g) == 0);
    expect(&g, 0, "/virtual/star*"); assert(!(g.gl_flags & GLOB_MAGCHAR)); globfree(&g);
    assert(glob("/virtual/star\\*", flags | GLOB_NOESCAPE, NULL, &g) == GLOB_NOMATCH);
    assert(g.gl_pathc == 0 && g.gl_pathv == NULL); globfree(&g);
    assert(glob("/virtual/{a,{sub,b}}", flags | GLOB_BRACE, NULL, &g) == 0);
    assert(g.gl_pathc == 3); expect(&g, 0, "/virtual/a"); expect(&g, 2, "/virtual/sub"); globfree(&g);
    assert(glob("/{virtual/{b,a},virtual/sub}", flags | GLOB_BRACE | GLOB_NOSORT, NULL, &g) == 0);
    assert(g.gl_pathc == 3); expect(&g, 0, "/virtual/b"); expect(&g, 1, "/virtual/a"); globfree(&g);
    g.gl_offs = 2;
    assert(glob("/virtual/b", flags | GLOB_DOOFFS, NULL, &g) == 0);
    assert(glob("/virtual/a", flags | GLOB_DOOFFS | GLOB_APPEND, NULL, &g) == 0);
    assert(!g.gl_pathv[0] && !g.gl_pathv[1]);
    expect(&g, 0, "/virtual/b"); expect(&g, 1, "/virtual/a");
    assert(glob("/missing", flags | GLOB_DOOFFS | GLOB_APPEND, NULL, &g) == GLOB_NOMATCH);
    assert(g.gl_pathc == 2); globfree(&g);
    assert(glob("/missing", flags | GLOB_NOCHECK, NULL, &g) == 0);
    expect(&g, 0, "/missing"); globfree(&g);
    assert(glob("/denied/*", flags | GLOB_ERR, on_error, &g) == GLOB_ABORTED);
    assert(errno == EACCES && error_calls == 1); globfree(&g);
    read_error = 1;
    int before = closes;
    assert(glob("/virtual/*", flags | GLOB_ERR, on_error, &g) == GLOB_ABORTED);
    assert(errno == EIO && closes == before + 1 && error_calls == 2); globfree(&g);
    read_error = 0;
    assert(glob("", flags, NULL, &g) == GLOB_NOMATCH);
    assert(g.gl_pathc == 0 && g.gl_pathv == NULL); globfree(&g);
    puts("GLOB_CALLBACKS_OK");
    return 0;
}
