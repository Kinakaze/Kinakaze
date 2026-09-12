extern long double powl(long double, long double);
extern int *__errno_location(void);
/* Calls the real ELF binary80 ABI; no Python/libffi long-double conversion. */
int evaluate(const unsigned char *xbytes, const unsigned char *ybytes,
             unsigned char *result, unsigned short *status) {
    union { long double value; unsigned char bytes[16]; } x, y, out;
    for (int i=0; i<16; ++i) { x.bytes[i]=xbytes[i]; y.bytes[i]=ybytes[i]; }
    unsigned short before, after;
    unsigned mxcsr, mxcsr_after;
    __asm__ volatile("fnstcw %0; fnclex" : "=m"(before));
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr));
    mxcsr &= ~63u;
    __asm__ volatile("ldmxcsr %0" : : "m"(mxcsr));
    *__errno_location()=0;
    long double (*volatile call)(long double,long double)=powl;
    out.value=call(x.value,y.value);
    __asm__ volatile("fnstcw %0; fnstsw %1" : "=m"(after), "=m"(*status));
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr_after));
    *status |= mxcsr_after & 63;
    for (int i=0; i<10; ++i) result[i]=out.bytes[i];
    for (int i=10; i<16; ++i) result[i]=0;
    return before!=after || mxcsr!=(mxcsr_after & ~63u) ? -1000 : *__errno_location();
}
