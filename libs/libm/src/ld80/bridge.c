/* Keep the guest System V floating-point ABI out of the Windows C ABI. */
#include "powl.c"

extern void __attribute__((ms_abi)) kinakaze_powl_set_errno(int);
void __attribute__((ms_abi)) kinakaze_powl80(const long double *x,
                                          const long double *y,
                                          long double *output) {
    unsigned short saved, extended;
    __asm__ volatile("fnstcw %0" : "=m"(saved));
    extended=(saved & ~0x0300) | 0x0300;
    __asm__ volatile("fldcw %0" : : "m"(extended) : "memory");
    long double result=kinakaze_internal_powl80(*x, *y);
    /* Input constrains the result to be evaluated before restoring precision. */
    __asm__ volatile("fldcw %0" : : "m"(saved), "m"(result) : "memory");
    union ld80_bits xb={.value=*x}, yb={.value=*y}, rb={.value=result};
    unsigned xe=xb.bits.exponent & 0x7fff, ye=yb.bits.exponent & 0x7fff;
    unsigned re=rb.bits.exponent & 0x7fff;
    if (xe!=0x7fff && ye!=0x7fff) {
        if (re==0x7fff) {
            kinakaze_powl_set_errno(rb.bits.significand==0x8000000000000000ULL ? 34 : 33);
        } else if (rb.bits.significand==0 && xb.bits.significand!=0) {
            kinakaze_powl_set_errno(34);
        }
    }
    *output=result;
}
