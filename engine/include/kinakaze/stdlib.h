#ifndef KINAKAZE_STDLIB_H
#define KINAKAZE_STDLIB_H

#include <kinakaze/types.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

#ifdef __cplusplus
extern "C" {
#endif

void *malloc(size_t size);
void free(void *value);
void *calloc(size_t count, size_t size);
void *realloc(void *value, size_t size);

/*
 * Aligned allocation. V8 asks for these through `base::AlignedAlloc`, and they
 * differ from `malloc` in a way that matters: the result must satisfy the
 * requested alignment, not merely the platform maximum.
 */
int posix_memalign(void **result, size_t alignment, size_t size);
void *aligned_alloc(size_t alignment, size_t size);
void *memalign(size_t alignment, size_t size);

void exit(int status) __attribute__((noreturn));
void _exit(int status) __attribute__((noreturn));
void abort(void) __attribute__((noreturn));
int atexit(void (*handler)(void));

int atoi(const char *text);
long atol(const char *text);
double atof(const char *text);
long strtol(const char *text, char **end, int base);
unsigned long strtoul(const char *text, char **end, int base);
double strtod(const char *text, char **end);

int abs(int value);
long labs(long value);

/*
 * The environment block: a null-terminated array of "KEY=VALUE" strings. Both
 * spellings name one variable, which is also what `getenv` reads, so walking the
 * array by hand and calling `getenv` cannot disagree.
 */
extern char **environ;
extern char **__environ;

char *getenv(const char *name);
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);

/*
 * The C++ form of `atexit`. `__cxa_finalize` runs and removes the handlers
 * belonging to one shared object; a null handle runs all of them. Handlers run in
 * reverse order of registration, interleaved with those from `atexit`.
 */
int __cxa_atexit(void (*function)(void *), void *argument, void *dso);
void __cxa_finalize(void *dso);
extern void *__dso_handle;

void qsort(void *base, size_t count, size_t size,
           int (*comparator)(const void *, const void *));
void *bsearch(const void *key, const void *base, size_t count, size_t size,
              int (*comparator)(const void *, const void *));

int posix_openpt(int flags);
int grantpt(int fd);
int unlockpt(int fd);
char *ptsname(int fd);
int ptsname_r(int fd, char *buf, size_t buflen);

#ifdef __cplusplus
}
#endif

#endif
