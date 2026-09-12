#ifndef KINAKAZE_ASSERT_H
#define KINAKAZE_ASSERT_H

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The failure path of `assert`. Prints a diagnostic naming the program, the
 * source location and the expression, then aborts. It does not return.
 */
void __assert_fail(const char *assertion, const char *file, unsigned int line,
                   const char *function) __attribute__((noreturn));

/*
 * The stack-protector failure path. The compiler emits this call when a frame's
 * guard value has been overwritten, so it does not return.
 */
void __stack_chk_fail(void) __attribute__((noreturn));

#ifdef __cplusplus
}
#endif

/*
 * NDEBUG compiles the checks out, as C requires. The expression is still
 * consumed by `sizeof` so that an unused-variable warning does not appear only in
 * release builds.
 */
#ifdef NDEBUG
#define assert(expression) ((void)sizeof((expression) ? 1 : 0))
#else
#define assert(expression)                                                     \
    ((expression) ? (void)0                                                    \
                  : __assert_fail(#expression, __FILE__, __LINE__,             \
                                  __func__))
#endif

#endif
