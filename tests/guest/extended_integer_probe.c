typedef unsigned long size_t;
extern long long llroundl(long double);
extern long lroundl(long double);
extern long double ceill(long double),floorl(long double),truncl(long double);
extern long long llrint(double),llrintf(float);
extern int fesetround(int),fegetround(void),feclearexcept(int),fetestexcept(int);
extern int *__errno_location(void);
extern long write(int,const void *,size_t);
extern void _exit(int) __attribute__((noreturn));
static void require(int ok,const char *message) {
    if(ok)return;size_t n=0;while(message[n])++n;write(2,message,n);write(2,"\n",1);_exit(93);
}
__attribute__((used,noinline)) static void run(void) {
    struct { long double input; long long expected; } cases[]={
        {0.0L,0},{-0.0L,0},{0x1p-16400L,0},{-0x1p-16400L,0},
        {0.5L,1},{-0.5L,-1},{2.5L,3},{-2.5L,-3},
        {2.5L-0x1p-62L,2},{-2.5L+0x1p-62L,-2},
        {9007199254740993.0L,9007199254740993LL},
        {9223372036854775807.0L,9223372036854775807LL},
        {-9223372036854775808.0L,(-9223372036854775807LL-1)}
    };
    int original=fegetround();
    for(int mode=0;mode<=0xc00;mode+=0x400) {
        require(!fesetround(mode),"set rounding mode");
        for(size_t i=0;i<sizeof(cases)/sizeof(cases[0]);++i) {
            feclearexcept(0x3d);*__errno_location()=71;
            require(llroundl(cases[i].input)==cases[i].expected,"llroundl exact extended input");
            require(lroundl(cases[i].input)==cases[i].expected,"Linux long is 64 bits");
            require(fetestexcept(0x3d)==0,"valid conversion does not raise inexact");
            require(fegetround()==mode && *__errno_location()==71,"conversion preserves rounding mode and errno");
        }
        require(ceill(1.0L+0x1p-63L)==2.0L,"ceill retains extended fraction");
        require(floorl(-1.0L-0x1p-63L)==-2.0L,"floorl retains extended fraction");
        require(truncl(-2.5L)==-2.0L,"truncl towards zero");
        require(ceill(9007199254740993.0L)==9007199254740993.0L,"ceill retains extended integer");
        require(fegetround()==mode,"integral operations restore caller control word");
        require(llrint(2.5)==(mode==0x800?3:2),"llrint obeys rounding mode");
        require(llrintf(-2.5f)==(mode==0x400?-3:-2),"llrintf obeys rounding mode");
    }
    union { long double value; struct { unsigned long significand; unsigned short exponent; } bits; } invalids[]={
        {.value=9223372036854775807.5L}, {.value=-9223372036854775809.0L},
        {.bits={0x8000000000000000UL,0x7fff}}, {.bits={0xc000000000000000UL,0x7fff}}
    };
    for(size_t i=0;i<sizeof(invalids)/sizeof(invalids[0]);++i) {
        feclearexcept(0x3d);*__errno_location()=71;
        volatile long long result=llroundl(invalids[i].value);(void)result;
        require(fetestexcept(0x3d)==1,"invalid conversion raises FE_INVALID only");
        require(*__errno_location()==71,"invalid conversion preserves errno");
    }
    fesetround(original);write(1,"EXTENDED_INTEGER_OK\n",20);_exit(0);
}
__attribute__((naked)) void _start(void) { __asm__("and $-16,%rsp\ncall run\nud2"); }
