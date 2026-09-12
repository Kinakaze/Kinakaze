typedef unsigned long size_t;
typedef int wchar_t;
extern int swprintf(wchar_t *, size_t, const wchar_t *, ...);
extern int vswprintf(wchar_t *, size_t, const wchar_t *, __builtin_va_list);
static int via_list(wchar_t *output, size_t length, const wchar_t *format, ...) {
    __builtin_va_list args;
    __builtin_va_start(args, format);
    int result = vswprintf(output, length, format, args);
    __builtin_va_end(args);
    return result;
}
static int equal(const wchar_t *a, const wchar_t *b) {
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}
int probe_wide_format(void) {
    wchar_t output[400];
    int count = -1;
    if (swprintf(output, 400, L"界:%4.2ls:%lc%n", L"你好吗", (wchar_t)0x1f680, &count) != 8) return 1;
    if (!equal(output, L"界:  你好:🚀") || count != 8) return 2;
    if (swprintf(output, 400, L"[%5.2s]", "你好呀") != 7 || !equal(output, L"[   你好]")) return 3;
    if (via_list(output, 400, L"%3$*1$.*2$ls/%4$.2f", 5, 2, L"你好呀", 1.25) != 10) return 4;
    if (!equal(output, L"   你好/1.25")) return 5;
    if (swprintf(output, 400, L"%d%d%d%d%d%d%d/%x", 1, 2, 3, 4, 5, 6, 7, 0xab) != 10 || !equal(output, L"1234567/ab")) return 6;
    wchar_t small[4] = { 99, 99, 99, 0x777777 };
    if (swprintf(small, 3, L"12345") >= 0 || small[2] != 0 || small[3] != 0x777777) return 7;
    if (swprintf(0, 0, L"abc") >= 0) return 8;
    if (swprintf(output, 400, L"%.1s", "A\xff") != 1 || !equal(output, L"A")) return 9;
    if (swprintf(output, 400, L"%s", "\xff") >= 0) return 10;
    wchar_t large[300];
    for (int i = 0; i < 299; ++i) large[i] = 0x754c;
    large[299] = 0;
    if (swprintf(output, 400, large) != 299 || !equal(output, large)) return 11;
    long n = -1;
    if (swprintf(output, 400, L"%lc%ln", (wchar_t)0x754c, &n) != 1 || n != 1) return 12;
    return 0;
}
