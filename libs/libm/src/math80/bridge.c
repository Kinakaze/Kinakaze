/* All entry points use pointers and the Windows ABI. Rust owns the ELF ABI. */
#include "complex_impl.h"

extern void __attribute__((ms_abi)) kinakaze_powl_set_errno(int);

void __attribute__((ms_abi)) kinakaze_real80(int operation, const long double *x,
                                           long double *output) {
    unsigned short saved = enter_math80();
    long double result = operation == 0 ? expm1l(*x) : cbrtl(*x);
    *output = result;
    leave_math80(saved);
    if (operation == 0 && isfinite(*x) &&
        (isinf(result) || (*x != 0 && fabsl(result) < 0x1p-16382L)))
        kinakaze_powl_set_errno(34);
}

/* Evaluate exp(x)*factor/2 without overflowing an intermediate exp(x). */
static long double half_exp_product(long double x, long double factor) {
    if (factor == 0) return factor;
    long double half = expl(x / 2);
    return (half * factor / 2) * half;
}

static long double sinh_small(long double x) {
    long double t = expm1l(fabsl(x));
    return copysignl((t + t / (t + 1)) / 2, x);
}

static long double complex hyperbolic(long double complex z, int sine) {
    long double x = creall(z), y = cimagl(z), ax = fabsl(x);
    if (y == 0) {
        if (ax < 22) {
            long double t = expm1l(ax);
            return CMPLXL(sine ? sinh_small(x) : 1 + t * t / (2 * (t + 1)),
                          sine ? y : copysignl(0, x) * y);
        }
        long double value = half_exp_product(ax, 1);
        return CMPLXL(sine ? copysignl(value, x) : value,
                      sine ? y : copysignl(0, x) * y);
    }
    if (!isfinite(y)) {
        if (isinf(x)) return CMPLXL(sine ? x : INFINITY, y - y);
        if (x == 0) return sine ? CMPLXL(x, y - y) : CMPLXL(y - y, x * copysignl(0, y));
        return CMPLXL(x + (y - y), x + (y - y));
    }
    long double s = sinl(y), c = cosl(y);
    if (ax < 22) {
        long double t = expm1l(ax), sh = sinh_small(x);
        long double ch = 1 + t * t / (2 * (t + 1));
        return sine ? CMPLXL(sh * c, ch * s) : CMPLXL(ch * c, sh * s);
    }
    long double real = half_exp_product(ax, c);
    long double imag = half_exp_product(ax, s);
    return sine ? CMPLXL(copysignl(1, x) * real, imag)
                : CMPLXL(real, copysignl(1, x) * imag);
}

static long double complex tangent_hyperbolic(long double complex z) {
    long double x = creall(z), y = cimagl(z), ax = fabsl(x);
    if (isinf(x))
        return CMPLXL(copysignl(1, x), copysignl(0, isfinite(y) ? sinl(y) * cosl(y) : y));
    if (isnan(x)) return CMPLXL(x + y, y == 0 ? y : x + y);
    if (!isfinite(y)) return CMPLXL(x == 0 ? x : y - y, y - y);
    long double s = sinl(y), c = cosl(y);
    if (ax >= 32) {
        long double e = expl(-ax);
        return CMPLXL(copysignl(1, x), 4 * s * c * e * e);
    }
    long double sh = sinh_small(x), ch = sqrtl(1 + sh * sh);
    long double denominator = sh * sh + c * c;
    return CMPLXL(sh * ch / denominator, s * c / denominator);
}

static long double complex power(long double complex z, long double complex w) {
    long double x = creall(z), y = cimagl(z), a = creall(w), b = cimagl(w);
    if (a == 0 && b == 0) return CMPLXL(1, 0);
    long double radius;
    if (fabsl(x) > 0x1p16382L || fabsl(y) > 0x1p16382L)
        radius = logl(hypotl(x / 2, y / 2)) + 0.693147180559945309417232121458176568L;
    else if (fabsl(x) >= 0.5L && fabsl(x) < 1.5L && fabsl(y) < 0.5L)
        radius = log1pl((fabsl(x) - 1) * (fabsl(x) + 1) + y * y) / 2;
    else
        radius = logl(hypotl(x, y));
    long double angle = atan2l(y, x);
    long double magnitude = expl(a * radius - (b == 0 ? 0 : b * angle));
    long double phase = a * angle + (b == 0 ? 0 : b * radius);
    long double s = sinl(phase), c = cosl(phase);
    return CMPLXL(c == 0 ? c : magnitude * c, s == 0 ? s : magnitude * s);
}

/* Operation numbers are shared with complex_extended.rs. Promoting the f32/f64
 * inputs keeps the same branch handling while each result is rounded exactly
 * once to its public format. Extended inputs never pass through binary64. */
void __attribute__((ms_abi)) kinakaze_complex80(int operation, int precision,
                                              const void *input, void *output) {
    unsigned short saved = enter_math80();
    long double values[4];
    int count = operation == 10 ? 4 : 2;
    for (int i = 0; i < count; ++i) {
        if (precision == 32) values[i] = ((const float *)input)[i];
        else if (precision == 64) values[i] = ((const double *)input)[i];
        else values[i] = ((const long double *)input)[i];
    }
    long double complex z = CMPLXL(values[0], values[1]), result;
    switch (operation) {
    case 0: result = cacosl(z); break;
    case 1: result = casinl(z); break;
    case 2: result = catanl(z); break;
    case 3: result = cacoshl(z); break;
    case 4: result = casinhl(z); break;
    case 5: result = catanhl(z); break;
    case 6: result = hyperbolic(z, 0); break;
    case 7: result = hyperbolic(z, 1); break;
    case 8: result = tangent_hyperbolic(z); break;
    case 9:
        result = tangent_hyperbolic(CMPLXL(values[1], values[0]));
        result = CMPLXL(cimagl(result), creall(result));
        break;
    case 10: result = power(z, CMPLXL(values[2], values[3])); break;
    case 11: result = CMPLXL(values[0], -values[1]); break;
    case 12:
        result = isinf(values[0]) || isinf(values[1])
            ? CMPLXL(INFINITY, copysignl(0, values[1])) : z;
        break;
    case 13: result = CMPLXL(atan2l(values[1], values[0]), 0); break;
    case 14: result = CMPLXL(values[0], 0); break;
    default: result = CMPLXL(values[1], 0); break;
    }
    if (precision == 32) {
        ((float *)output)[0] = creall(result);
        ((float *)output)[1] = cimagl(result);
    } else if (precision == 64) {
        ((double *)output)[0] = creall(result);
        ((double *)output)[1] = cimagl(result);
    } else {
        ((long double *)output)[0] = creall(result);
        ((long double *)output)[1] = cimagl(result);
    }
    leave_math80(saved);
}
