/* Adapt the FreeBSD complex inverse routines to the private C binary80 ABI. */
#ifndef KINAKAZE_COMPLEX80_H
#define KINAKAZE_COMPLEX80_H
#include "libm.h"
#define complex _Complex
#define CMPLXL(x, y) __builtin_complex((long double)(x), (long double)(y))
#define creall(z) (__real__(z))
#define cimagl(z) (__imag__(z))
#define __unused __attribute__((unused))
#define nan_mix(x, y) ((x) + (y))
#define GET_LDBL_EXPSIGN(output, input) do { \
    union ldshape bits = {.f = (input)}; (output) = bits.i.se; \
} while (0)
#define SET_LDBL_EXPSIGN(output, exponent) do { \
    union ldshape bits = {.f = (output)}; bits.i.se = (exponent); (output) = bits.f; \
} while (0)
union IEEEl2bits { long double e; };
#define LD80C(mantissa, exponent, value) { .e = (value) }
static const long double pio2_hi = 0x1.921fb54442d1846ap+0L;
static const long double pio2_lo = -2.50827880633416601173e-20L;
#define cacosl kinakaze_math80_cacos
#define casinl kinakaze_math80_casin
#define catanl kinakaze_math80_catan
#define cacoshl kinakaze_math80_cacosh
#define casinhl kinakaze_math80_casinh
#define catanhl kinakaze_math80_catanh
long double complex cacosl(long double complex);
long double complex casinl(long double complex);
long double complex catanl(long double complex);
long double complex cacoshl(long double complex);
long double complex casinhl(long double complex);
long double complex catanhl(long double complex);
#endif
