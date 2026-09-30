/* Private binary80 support. No host CRT long-double ABI crosses this boundary. */
#ifndef KINAKAZE_MATH80_H
#define KINAKAZE_MATH80_H
#include <stdint.h>
#include "../ld80/libm.h"

typedef float float_t;
typedef double double_t;
#define M_PI_4 0.785398163397448309615660845819875721
union ldshape {
    long double f;
    struct { uint64_t m; uint16_t se; } i;
};
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define copysignl(x, y) __builtin_copysignl(x, y)
#define predict_false(x) __builtin_expect(!!(x), 0)
#define FORCE_EVAL(x) do { volatile __typeof__(x) value = (x); (void)value; } while (0)

/* Prefix every separately compiled C definition, including internal kernels. */
#define expm1l kinakaze_math80_expm1
#define cbrtl kinakaze_math80_cbrt
#define expl kinakaze_math80_exp
#define sinl kinakaze_math80_sin
#define cosl kinakaze_math80_cos
#define __sinl kinakaze_math80_sin_kernel
#define __cosl kinakaze_math80_cos_kernel
#define __rem_pio2l kinakaze_math80_reduce
#define __rem_pio2_large kinakaze_math80_reduce_large
long double expm1l(long double);
long double cbrtl(long double);
long double expl(long double);
long double sinl(long double);
long double cosl(long double);
long double __sinl(long double, long double, int);
long double __cosl(long double, long double);
int __rem_pio2l(long double, long double *);
int __rem_pio2_large(double *, double *, int, int, int);

/* The large-angle reducer only uses these on bounded binary64 values. */
static inline double floor(double x) { return floorl(x); }
static inline double scalbn(double x, int n) { return scalbnl(x, n); }
static inline long double sqrtl(long double x) {
    long double result;
    __asm__("fsqrt" : "=t"(result) : "0"(x));
    return result;
}
static inline long double atan2l(long double y, long double x) {
    long double result;
    __asm__("fpatan" : "=t"(result) : "0"(x), "u"(y) : "st(1)");
    return result;
}
static inline long double atanl(long double x) { return atan2l(x, 1); }
static inline long double asinl(long double x) {
    return atan2l(x, sqrtl((1 - x) * (1 + x)));
}
static inline long double acosl(long double x) {
    return atan2l(sqrtl((1 - x) * (1 + x)), x);
}
static inline long double logl(long double x) {
    long double result;
    __asm__("fldln2; fxch; fyl2x" : "=t"(result) : "0"(x) : "st(1)");
    return result;
}
static inline long double log1pl(long double x) {
    if (fabsl(x) >= 0.25L) return logl(1 + x);
    long double result;
    __asm__("fldln2; fxch; fyl2xp1" : "=t"(result) : "0"(x) : "st(1)");
    return result;
}
static inline long double atanhl(long double x) {
    long double a = fabsl(x);
    return copysignl(0.5L * log1pl(2 * a / (1 - a)), x);
}
static inline long double hypotl(long double x, long double y) {
    x = fabsl(x); y = fabsl(y);
    if (isinf(x) || isinf(y)) return INFINITY;
    if (isnan(x) || isnan(y)) return x + y;
    if (x < y) { long double temporary = x; x = y; y = temporary; }
    if (y == 0) return x;
    int exponent;
    long double a = frexpl(x, &exponent), b = scalbnl(y, -exponent);
    return scalbnl(sqrtl(a * a + b * b), exponent);
}

/* Windows initializes x87 at binary64 precision; Linux long double needs 64
 * significand bits. Preserve the caller's rounding mode and exception masks. */
static inline unsigned short enter_math80(void) {
    unsigned short saved, extended;
    __asm__ volatile("fnstcw %0" : "=m"(saved));
    extended = (saved & ~0x0300) | 0x0300;
    __asm__ volatile("fldcw %0" : : "m"(extended) : "memory");
    return saved;
}
static inline void leave_math80(unsigned short saved) {
    __asm__ volatile("fldcw %0" : : "m"(saved) : "memory");
}
#endif
