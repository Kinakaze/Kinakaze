/* Private helpers for the vendored IEEE binary80 power implementation. */
#define LDBL_MANT_DIG 64
#define LDBL_MAX_EXP 16384
#define LDBL_MIN_EXP (-16381)
#define LDBL_EPSILON 0x1p-63L
#define INFINITY (__builtin_inff())
#define isnan(x) __builtin_isnan(x)
#define signbit(x) __builtin_signbit(x)
#define fabsl(x) __builtin_fabsl(x)
#define powl kinakaze_internal_powl80

_Static_assert(sizeof(long double) == 16 && __LDBL_MANT_DIG__ == 64,
               "powl requires IEEE binary80 storage, not Windows double");
union ld80_bits {
    long double value;
    struct { unsigned long long significand; unsigned short exponent; } bits;
};
static long double floorl(long double x) {
    unsigned short saved, rounding;
    long double result;
    __asm__ volatile("fnstcw %0" : "=m"(saved));
    rounding = (saved & ~0x0c00) | 0x0400;
    __asm__ volatile("fldcw %2; fldt %1; frndint; fstpt %0; fldcw %3"
                     : "=m"(result) : "m"(x), "m"(rounding), "m"(saved)
                     : "st", "memory");
    return result;
}
static long double frexpl(long double x, int *exponent) {
    union ld80_bits value = {.value=x};
    unsigned exp = value.bits.exponent & 0x7fff;
    *exponent=0;
    if (exp==0x7fff || (exp==0 && value.bits.significand==0)) return x;
    if (exp==0) {
        int power=-16381;
        while (!(value.bits.significand >> 63)) {
            value.bits.significand <<= 1;
            --power;
        }
        *exponent=power;
    } else *exponent=(int)exp-16382;
    value.bits.exponent=(value.bits.exponent & 0x8000) | 16382;
    return value.value;
}
static long double scalbnl(long double x, int exponent) {
    long double result;
    __asm__ volatile("fildl %2; fldt %1; fscale; fstp %%st(1); fstpt %0"
                     : "=m"(result) : "m"(x), "m"(exponent)
                     : "st", "st(1)");
    return result;
}
static long double __polevll(long double x, const long double *coeff, int degree) {
    long double result=*coeff++;
    while (degree--) result=result*x+*coeff++;
    return result;
}
static long double __p1evll(long double x, const long double *coeff, int degree) {
    long double result=x+*coeff++;
    while (--degree) result=result*x+*coeff++;
    return result;
}
