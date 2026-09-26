/* SysV varargs, Linux wchar_t counts and actual wide FILE output. */
typedef unsigned long size_t;
typedef int wchar_t;
typedef void FILE;
extern char *setlocale(int, const char *);
extern FILE *fopen(const char *, const char *);
extern int fclose(FILE *), fflush(FILE *), ferror(FILE *), fputs(const char *, FILE *);
extern wchar_t *fgetws(wchar_t *, int, FILE *);
extern int fwprintf(FILE *, const wchar_t *, ...);
extern int wprintf(const wchar_t *, ...);
extern int vfwprintf(FILE *, const wchar_t *, __builtin_va_list);
extern int vwprintf(const wchar_t *, __builtin_va_list);
extern int __fwprintf_chk(FILE *, int, const wchar_t *, ...);
extern int __wprintf_chk(int, const wchar_t *, ...);
extern int __vfwprintf_chk(FILE *, int, const wchar_t *, __builtin_va_list);
extern int __vwprintf_chk(int, const wchar_t *, __builtin_va_list);
extern int __swprintf_chk(wchar_t *, size_t, int, size_t, const wchar_t *, ...);
extern long write(int, const void *, size_t);
extern void _exit(int);
extern int probe_wide_format(void);

static int stream_equal(const wchar_t *a, const wchar_t *b) {
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}
static int stream_via_list(FILE *file, int checked, const wchar_t *format, ...) {
    __builtin_va_list args;
    __builtin_va_start(args, format);
    int result = file ? (checked ? __vfwprintf_chk(file, 1, format, args) : vfwprintf(file, format, args))
                      : (checked ? __vwprintf_chk(1, format, args) : vwprintf(format, args));
    __builtin_va_end(args);
    return result;
}
static int run(void) {
    if (!setlocale(6, "C.UTF-8")) return 1;
    int previous = probe_wide_format();
    if (previous) return 30 + previous;
    FILE *file = fopen("/tmp/wide-printf", "w");
    if (!file) return 2;
    int count = -1;
    if (fwprintf(file, L"界:%4.2ls:%lc%n/%d%d%d%d%d%d%d/%.2f\n", L"你好吗", 0x1f680,
                 &count, 1, 2, 3, 4, 5, 6, 7, 1.25) != 22 || count != 8) return 3;
    if (__fwprintf_chk(file, 1, L"%2$*1$d\n", 4, 7) != 5) return 4;
    if (stream_via_list(file, 0, L"v=%ls %d\n", L"你", 42) != 7) return 5;
    if (stream_via_list(file, 1, L"c=%lc\n", 0x754c) != 4) return 6;
    if (fclose(file)) return 7;
    file = fopen("/tmp/wide-printf", "r");
    if (!file) return 8;
    wchar_t line[128];
    if (!fgetws(line, 128, file) || !stream_equal(line, L"界:  你好:🚀/1234567/1.25\n")) return 9;
    if (!fgetws(line, 128, file) || !stream_equal(line, L"   7\n")) return 10;
    if (!fgetws(line, 128, file) || !stream_equal(line, L"v=你 42\n")) return 11;
    if (!fgetws(line, 128, file) || !stream_equal(line, L"c=界\n")) return 12;
    if (fclose(file)) return 13;
    file = fopen("/tmp/wide-byte", "w");
    if (!file || fputs("byte", file) < 0 || fwprintf(file, L"wide") >= 0) return 14;
    fclose(file);
    file = fopen("/tmp/wide-invalid", "w");
    if (!file || fwprintf(file, L"%lc", 0x110000) >= 0 || !ferror(file)) return 15;
    fclose(file);
    if (fwprintf((FILE *)0, L"bad") >= 0) return 16;
    if (__swprintf_chk(line, 128, 1, 128, L"%ls %.2f %d%d%d%d%d%d", L"界", 1.25,
                       1, 2, 3, 4, 5, 6) != 13 || !stream_equal(line, L"界 1.25 123456")) return 17;
    if (wprintf(L"%ls\n", L"WIDE") != 5) return 18;
    if (stream_via_list((FILE *)0, 0, L"%ls\n", L"VA") != 3) return 19;
    if (stream_via_list((FILE *)0, 1, L"%ls\n", L"CHECKED_VA") != 11) return 20;
    if (__wprintf_chk(1, L"%ls %.2f\n", L"CHECKED", 1.25) != 13) return 21;
    if (fflush((FILE *)0)) return 22;
    return 0;
}
__attribute__((used, noinline)) static void entry(void) {
    int code = run();
    if (!code) {
        static const char marker[] = "WIDE_PRINTF_ABI_STREAMS_OK\n";
        write(1, marker, sizeof(marker) - 1);
    }
    _exit(code);
}
__attribute__((naked)) void _start(void) {
    __asm__ volatile("andq $-16, %rsp; call entry; ud2");
}
