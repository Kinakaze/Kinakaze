extern long double log1pl(long double);
extern int *__errno_location(void);

int evaluate(const unsigned char *input, unsigned char *output, unsigned short *status) {
    union { long double value; unsigned char bytes[16]; } argument, result;
    for (int index = 0; index < 16; ++index) argument.bytes[index] = input[index];
    unsigned short before, after;
    __asm__ volatile("fnstcw %0; fnclex" : "=m"(before));
    *__errno_location() = 0;
    long double (*volatile calculate)(long double) = log1pl;
    result.value = calculate(argument.value);
    __asm__ volatile("fnstcw %0; fnstsw %1" : "=m"(after), "=m"(*status));
    for (int index = 0; index < 10; ++index) output[index] = result.bytes[index];
    for (int index = 10; index < 16; ++index) output[index] = 0;
    return before == after ? *__errno_location() : -1000;
}
