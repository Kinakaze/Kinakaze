#ifndef KINAKAZE_MATH_H
#define KINAKAZE_MATH_H

#define HUGE_VAL (__builtin_huge_val())
#define HUGE_VALF (__builtin_huge_valf())
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))

#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4

double sin(double value);
float sinf(float value);
double cos(double value);
float cosf(float value);
double tan(double value);
float tanf(float value);
double asin(double value);
float asinf(float value);
double acos(double value);
float acosf(float value);
double atan(double value);
float atanf(float value);
double atan2(double y, double x);
float atan2f(float y, float x);

double sinh(double value);
float sinhf(float value);
double cosh(double value);
float coshf(float value);
double tanh(double value);
float tanhf(float value);
double asinh(double value);
float asinhf(float value);
double acosh(double value);
float acoshf(float value);
double atanh(double value);
float atanhf(float value);

double exp(double value);
float expf(float value);
double exp2(double value);
float exp2f(float value);
double expm1(double value);
float expm1f(float value);
double log(double value);
float logf(float value);
double log2(double value);
float log2f(float value);
double log10(double value);
float log10f(float value);
double log1p(double value);
float log1pf(float value);

double pow(double base, double exponent);
float powf(float base, float exponent);
double sqrt(double value);
float sqrtf(float value);
double cbrt(double value);
float cbrtf(float value);
double hypot(double x, double y);
float hypotf(float x, float y);

double ceil(double value);
float ceilf(float value);
double floor(double value);
float floorf(float value);
double round(double value);
float roundf(float value);
double trunc(double value);
float truncf(float value);
double rint(double value);
float rintf(float value);
double nearbyint(double value);
float nearbyintf(float value);

double fmod(double x, double y);
float fmodf(float x, float y);
double remainder(double x, double y);
float remainderf(float x, float y);

int isnan(double value);
int isnanf(float value);
int isinf(double value);
int isinff(float value);
int isfinite(double value);
int isfinitef(float value);
int signbit(double value);
int signbitf(float value);
int fpclassify(double value);
int fpclassifyf(float value);

double fdim(double x, double y);
float fdimf(float x, float y);
double fma(double x, double y, double z);
float fmaf(float x, float y, float z);
double ldexp(double value, int exponent);
float ldexpf(float value, int exponent);
double frexp(double value, int *exponent);
float frexpf(float value, int *exponent);
double modf(double value, double *integral);
float modff(float value, float *integral);
double nan(const char *tag);
float nanf(const char *tag);
double erf(double value);
float erff(float value);
double erfc(double value);
float erfcf(float value);
double lgamma(double value);
float lgammaf(float value);
double tgamma(double value);
float tgammaf(float value);

double fabs(double value);
float fabsf(float value);
double copysign(double magnitude, double sign);
float copysignf(float magnitude, float sign);
double fmin(double left, double right);
float fminf(float left, float right);
double fmax(double left, double right);
float fmaxf(float left, float right);

#endif
