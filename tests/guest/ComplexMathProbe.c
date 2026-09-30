/* Freestanding Linux ELF regression: real versioned imports and SysV calls. */
typedef unsigned long size_t;
extern long write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));
extern int *__errno_location(void);
extern void *dlopen(const char *, int);
extern void *dlsym(void *, const char *);
extern void *dlvsym(void *, const char *, const char *);
extern long double expm1l(long double), cbrtl(long double);
#define COMPLEX(T, x, y) __builtin_complex((T)(x), (T)(y))
#define DECLARE(name) \
    extern double _Complex name(double _Complex); \
    extern float _Complex name##f(float _Complex); \
    extern long double _Complex name##l(long double _Complex);
DECLARE(cacos) DECLARE(casin) DECLARE(catan)
DECLARE(cacosh) DECLARE(casinh) DECLARE(catanh)
DECLARE(ccosh) DECLARE(csinh) DECLARE(ctanh) DECLARE(ctan)
DECLARE(conj) DECLARE(cproj)
extern double _Complex cpow(double _Complex, double _Complex);
extern float _Complex cpowf(float _Complex, float _Complex);
extern long double _Complex cpowl(long double _Complex, long double _Complex);
extern float cargf(float _Complex), crealf(float _Complex), cimagf(float _Complex);
extern double creal(double _Complex), cimag(double _Complex);
extern long double cargl(long double _Complex), creall(long double _Complex), cimagl(long double _Complex);

static void require(int condition, const char *message) {
    if (condition) return;
    size_t length = 0;
    while (message[length]) ++length;
    write(2, message, length); write(2, "\n", 1); _exit(1);
}
static void close_to(long double actual, long double expected, long double tolerance, const char *message) {
    require(__builtin_fabsl(actual - expected) <= tolerance * (1 + __builtin_fabsl(expected)), message);
}
static void complex_close(long double _Complex actual, long double real, long double imag,
                          long double tolerance, const char *message) {
    close_to(__real__ actual, real, tolerance, message);
    close_to(__imag__ actual, imag, tolerance, message);
}

/* Reference values calculated independently with mpmath at 80 decimal digits. */
struct inverse_case {
    const char *name;
    double _Complex (*normal)(double _Complex);
    float _Complex (*single)(float _Complex);
    long double _Complex (*extended)(long double _Complex);
    long double real, imag;
};
static const struct inverse_case cases[] = {
    {"cacos", cacos, cacosf, cacosl, 1.17251845325658827471932372270912637161792L, -0.743320426325278466237037614609445583162392L},
    {"casin", casin, casinf, casinl, 0.39827787353830834451199796893062507048066L, 0.743320426325278466237037614609445583162392L},
    {"catan", catan, catanf, catanl, 0.692724188399600927172647865205603398791659L, 0.590213500279505364885927451450717532708594L},
    {"cacosh", cacosh, cacoshf, cacoshl, 0.743320426325278466237037614609445583162392L, 1.17251845325658827471932372270912637161792L},
    {"casinh", casinh, casinhf, casinhl, 0.606334999887351308436807977516308144864254L, 0.682203965583432313997227375432835325585929L},
    {"catanh", catanh, catanhf, catanhl, 0.310428283077195755334440591376480111003829L, 0.723220666124067592099983421237940208262707L},
    {"ccosh", ccosh, ccoshf, ccoshl, 0.825071366994607263464010117700068663546691L, 0.355198757890738464071882544462182772707524L},
    {"csinh", csinh, csinhf, csinhl, 0.381279634652178149802983947255653801357559L, 0.768633564693392754814289016734235678198482L},
    {"ctanh", ctanh, ctanhf, ctanhl, 0.728211801280472388383756931069788636320188L, 0.618096394806202169062795053441594505269097L},
    {"ctan", ctan, ctanf, ctanl, 0.290893461829618098380361265136057761383441L, 0.736084170551190969269516656891974834262884L},
};

static void versioned_symbols(void) {
    void *libc = dlopen("libc.so.6", 2), *math = dlopen("libm.so.6", 2);
    void *loader = dlopen("ld-linux-x86-64.so.2", 2);
    require(libc && math && loader, "load all symbol providers");
    static const char *common[] = {"__cxa_atexit", "__errno_location", "backtrace", "calloc"};
    for (unsigned i = 0; i < sizeof(common) / sizeof(*common); ++i)
        require(dlvsym(libc, common[i], "GLIBC_2.2.5") != 0, common[i]);
    require(dlvsym(libc, "__ctype_b_loc", "GLIBC_2.3") != 0, "__ctype_b_loc@GLIBC_2.3");
    require(dlvsym(libc, "__ctype_tolower_loc", "GLIBC_2.3") != 0, "__ctype_tolower_loc@GLIBC_2.3");
    require(dlvsym(loader, "__tls_get_addr", "GLIBC_2.3") != 0, "__tls_get_addr@GLIBC_2.3");
    static const char *names[] = {
        "cacos",
        "cacosf",
        "cacosl",
        "casin",
        "casinf",
        "casinl",
        "catan",
        "catanf",
        "catanl",
        "cacosh",
        "cacoshf",
        "cacoshl",
        "casinh",
        "casinhf",
        "casinhl",
        "catanh",
        "catanhf",
        "catanhl",
        "ccosh",
        "ccoshf",
        "ccoshl",
        "csinh",
        "csinhf",
        "csinhl",
        "ctanh",
        "ctanhf",
        "ctanhl",
        "ctan",
        "ctanf",
        "ctanl",
        "cpow",
        "cpowf",
        "cpowl",
        "conj",
        "conjf",
        "conjl",
        "cproj",
        "cprojf",
        "cprojl",
        "creal",
        "crealf",
        "creall",
        "cimag",
        "cimagf",
        "cimagl",
        "cargf",
        "cargl",
        "expm1l",
        "cbrtl",
    };
    for (unsigned i = 0; i < sizeof(names) / sizeof(*names); ++i) {
        void *address = dlvsym(math, names[i], "GLIBC_2.2.5");
        require(address && address == dlsym(math, names[i]), names[i]);
        require(!dlvsym(math, names[i], "GLIBC_99.99"), "reject nonexistent version");
    }
}

static void real_math(void) {
    long double x = 0x1p-80L;
    require(expm1l(x) == x && expm1l(-x) == -x, "expm1l preserves tiny arguments");
    require(__builtin_signbit(expm1l(-0.0L)), "expm1l negative zero");
    require(expm1l(-__builtin_infl()) == -1, "expm1l negative infinity");
    require(expm1l(__builtin_infl()) == __builtin_infl(), "expm1l positive infinity");
    require(__builtin_isnan(expm1l(__builtin_nanl(""))), "expm1l NaN");
    close_to(expm1l(1), 1.7182818284590452353602874713526625L, 2e-19L, "expm1l one");
    require(expm1l(11356.5L) < __builtin_infl(), "expm1l near upper boundary");
    require(expm1l(1000) > 0x1p1024L, "expm1l extended exponent range");
    *__errno_location() = 0;
    require(__builtin_isinf(expm1l(12000)) && *__errno_location() == 34, "expm1l overflow errno");
    *__errno_location() = 71;
    (void)expm1l(0.5L);
    require(*__errno_location() == 71, "expm1l preserves errno on success");
    require(cbrtl(8) == 2 && cbrtl(-8) == -2, "cbrtl real cube root");
    require(__builtin_signbit(cbrtl(-0.0L)), "cbrtl negative zero");
    require(cbrtl(0x1p12000L) == 0x1p4000L, "cbrtl extended exponent range");
    require(cbrtl(0x1p-16443L) == 0x1p-5481L, "cbrtl subnormal input");
    require(cbrtl(1 + 0x1p-60L) > 1, "cbrtl retains extended significand");
}

static void complex_math(void) {
    for (unsigned i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
        const struct inverse_case *c = cases + i;
        complex_close(c->normal(COMPLEX(double, 0.5, 0.75)), c->real, c->imag, 4e-16L, c->name);
        complex_close(c->single(COMPLEX(float, 0.5, 0.75)), c->real, c->imag, 8e-8L, c->name);
        complex_close(c->extended(COMPLEX(long double, 0.5, 0.75)), c->real, c->imag, 8e-19L, c->name);
    }
    long double _Complex z;
    z = cacosl(COMPLEX(long double, 2, 0.0L));
    require(__real__ z == 0 && __imag__ z < 0, "cacosl upper branch cut");
    z = cacosl(COMPLEX(long double, 2, -0.0L));
    require(__real__ z == 0 && __imag__ z > 0, "cacosl lower branch cut");
    z = casinl(COMPLEX(long double, 0x1p-80L, -0.0L));
    require(__real__ z == 0x1p-80L && __builtin_signbit(__imag__ z), "casinl tiny and signed zero");
    z = casinhl(COMPLEX(long double, 0x1p12000L, 0x1p12000L));
    require(__builtin_isfinite(__real__ z) && __builtin_isfinite(__imag__ z), "casinhl huge finite input");
    z = catanhl(COMPLEX(long double, 0x1p12000L, 0x1p12000L));
    require(__real__ z > 0 && __real__ z < 0x1p-10000L, "catanhl scaled reciprocal");
    z = cacoshl(COMPLEX(long double, __builtin_infl(), __builtin_infl()));
    require(__builtin_isinf(__real__ z) && __imag__ z > 0, "cacosh infinity");
    z = ccoshl(COMPLEX(long double, 12000, 0));
    require(__builtin_isinf(__real__ z) && __imag__ z == 0, "ccosh overflow preserves zero imaginary part");
    z = ctanhl(COMPLEX(long double, 12000, 1));
    require(__real__ z == 1 && __imag__ z == 0, "ctanh huge real input");
    complex_close(cpow(COMPLEX(double, 1, 1), COMPLEX(double, 2, 0)), 0, 2, 5e-16L, "cpow two complex ABI arguments");
    complex_close(cpowf(COMPLEX(float, 1, 1), COMPLEX(float, 2, 0)), 0, 2, 1e-7L, "cpowf packed arguments");
    complex_close(cpowl(COMPLEX(long double, 1, 1), COMPLEX(long double, 2, 0)), 0, 2, 8e-19L, "cpowl stack arguments");
    z = cpowl(COMPLEX(long double, 1 + 0x1p-60L, 0), COMPLEX(long double, 2, 0));
    require(__real__ z > 1 && __imag__ z == 0, "cpowl retains extended precision");
    complex_close(conj(COMPLEX(double, 2, -3)), 2, 3, 0, "conj");
    complex_close(conjf(COMPLEX(float, 2, -3)), 2, 3, 0, "conjf");
    complex_close(conjl(COMPLEX(long double, 2, -3)), 2, 3, 0, "conjl");
    complex_close(cproj(COMPLEX(double, 2, 3)), 2, 3, 0, "cproj finite");
    complex_close(cprojf(COMPLEX(float, 2, 3)), 2, 3, 0, "cprojf finite");
    z = cprojl(COMPLEX(long double, 0, -__builtin_infl()));
    require(__builtin_isinf(__real__ z) && __imag__ z == 0 && __builtin_signbit(__imag__ z), "cprojl infinity");
    close_to(cargf(COMPLEX(float, 1, 1)), 0.78539816339744830961566L, 1e-7L, "cargf");
    close_to(cargl(COMPLEX(long double, 1, 1)), 0.78539816339744830961566L, 8e-20L, "cargl");
    require(creal(COMPLEX(double, 2, 3)) == 2 && cimag(COMPLEX(double, 2, 3)) == 3, "double components");
    require(crealf(COMPLEX(float, 2, 3)) == 2 && cimagf(COMPLEX(float, 2, 3)) == 3, "float components");
    require(creall(COMPLEX(long double, 2, 3)) == 2 && cimagl(COMPLEX(long double, 2, 3)) == 3, "long double components");
}

__attribute__((noreturn)) void probe_start(void) {
    versioned_symbols();
    unsigned short before, after;
    __asm__ volatile("fnstcw %0" : "=m"(before));
    real_math();
    for (int i = 0; i < 20; ++i) complex_math();
    __asm__ volatile("fnstcw %0" : "=m"(after));
    require(before == after, "math restores caller x87 control word");
    write(1, "COMPLEX_MATH_OK\n", 16); _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");
}
