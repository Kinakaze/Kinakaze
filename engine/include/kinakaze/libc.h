#ifndef KINAKAZE_LIBC_H
#define KINAKAZE_LIBC_H

#include <kinakaze/assert.h>
#include <kinakaze/errno.h>
#include <kinakaze/stdlib.h>
#include <kinakaze/unistd.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The program entry point a glibc `crt1.o` calls instead of `main`. It publishes
 * the environment, registers `fini` and `rtld_fini` to run at exit, calls `init`,
 * calls `main`, and exits with what `main` returned. It does not return.
 *
 * `argv` must hold `argc` entries followed by a null, with the environment
 * directly above it, which is the layout a loader-built stack has. Any of the
 * function pointers may be null.
 */
int __libc_start_main(int (*main)(int, char **, char **), int argc, char **argv,
                      void (*init)(int, char **, char **), void (*fini)(void),
                      void (*rtld_fini)(void), void *stack_end);

#ifdef __cplusplus
}
#endif

#endif

