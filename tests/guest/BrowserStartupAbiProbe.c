extern long double powl(long double, long double);
extern long double sqrtl(long double);

extern int pthread_create(unsigned long *, const void *, void *(*)(void *), void *);
extern int pthread_join(unsigned long, void **);
extern int pthread_mutex_init(void *, const void *);
extern int pthread_mutex_destroy(void *);
extern int pthread_mutex_lock(void *);
extern int pthread_mutex_unlock(void *);
extern int pthread_mutex_trylock(void *);
extern int fork(void);
extern int waitpid(int, int *, int);
extern void _exit(int);
extern int usleep(unsigned);
extern long syscall(long, ...);
static unsigned long gs_values[4] = {11, 22, 33, 44};
// No dynamic symbol and only one INT3 before this function. Its unwind entry
// is the authoritative root; case bodies are reached solely by a jump table.
__asm__(
    ".pushsection .text.browser_hidden,\"ax\",@progbits\n"
    ".byte 0x90,0xc3,0xcc\n"
    ".local browser_hidden_switch\n.type browser_hidden_switch,@function\n"
    "browser_hidden_switch:\n.cfi_startproc\n"
    "lea .Lbrowser_switch_table(%rip),%r11\njmp 8f\n"
    "8: cmp $2,%edi\nja 9f\n"
    "movslq (%r11,%rdi,4),%rax\nadd %r11,%rax\n"
    "mov $0x11,%r10d\nmov $0x22,%r9d\njmp *%rax\n"
    "1: mov %gs:0,%rax\nret\n2: mov %gs:8,%rax\nret\n"
    "3: mov %gs:16,%rax\nret\n9: xor %eax,%eax\nret\n"
    ".cfi_endproc\n.size browser_hidden_switch,.-browser_hidden_switch\n.popsection\n"
    ".pushsection .rodata\n.Lbrowser_switch_table:\n"
    ".long 1b-.Lbrowser_switch_table,2b-.Lbrowser_switch_table,3b-.Lbrowser_switch_table\n.popsection\n"
);
int browser_gs_unwind_probe(void) {
    unsigned long old;
    if (syscall(158,0x1004,&old) || syscall(158,0x1001,gs_values)) return 1;
    unsigned long (*indirect)(unsigned);
    __asm__ volatile("lea browser_hidden_switch(%%rip),%0" : "=r"(indirect));
    int result = 0;
    for (unsigned i=0;i<3;i++) if (indirect(i)!=gs_values[i]) result=2+i;
    if (indirect(3)!=0) result=5;
    return syscall(158,0x1001,old) ? 6 : result;
}
int browser_fs_redzone_probe(void) {
    unsigned char bad;
    __asm__ volatile("movq $123, -8(%%rsp); mov %%fs:0x28, %%rax; cmpq $123, -8(%%rsp); setne %0"
        : "=q"(bad) :: "rax", "cc", "memory");
    return bad;
}
static unsigned long other_gs[4] = {55, 66, 77, 88};
static void *gs_thread(void *argument) {
    unsigned long got;
    if (syscall(158, 0x1004, &got) || got != (unsigned long)gs_values) return (void *)2;
    if (syscall(158, 0x1001, other_gs)) return (void *)1;
    __asm__ volatile("mov %%gs:8, %0" : "=r"(got));
    return (void *)(unsigned long)(got != 66);
}
int browser_gs_probe(void) {
    unsigned long old = 0, got = 0, base = 0;
    if (syscall(158, 0x1004, &old) || syscall(158, 0x1001, gs_values)) return 1;
    if (syscall(158, 0x1004, &base) || base != (unsigned long)gs_values) return 2;
    __asm__ volatile("mov %%gs:8, %0" : "=r"(got));
    if (got != 22) return 3;
    // 32-bit effective addresses must be zero-extended before adding GS.base.
    __asm__ volatile("mov $8, %%eax; mov %%gs:(%%eax), %0" : "=r"(got) :: "rax");
    if (got != 22) return 4;
    __asm__ volatile("movq $99, %%gs:16" ::: "memory");
    if (gs_values[2] != 99) return 5;
    // This access is only four bytes: the following entry must stay intact.
    __asm__ volatile("xor %%eax, %%eax; mov %%gs:(%%rax), %%rdx" : "=d"(got) :: "rax", "cc");
    if (got != 11) return 6;
    unsigned char redzone_bad, flags_bad;
    __asm__ volatile("movq $123, -8(%%rsp); xor %%eax, %%eax; stc; "
        "mov %%gs:(%%rax), %%rdx; setnc %1; cmpq $123, -8(%%rsp); setne %0"
        : "=q"(redzone_bad), "=q"(flags_bad), "=d"(got) :: "rax", "cc", "memory");
    if (got != 11 || redzone_bad || flags_bad) return 12;
    __asm__ volatile("wrgsbase %0; rdgsbase %1" : "+r"(base), "=r"(got) :: "memory");
    if (got != base) return 7;
    __asm__ volatile("mov $0x1234, %%eax; wrgsbase %%eax; rdgsbase %0"
        : "=r"(got) :: "rax", "memory");
    if (got != 0x1234 || syscall(158, 0x1001, base)) return 13;
    unsigned long thread; void *result;
    if (pthread_create(&thread, 0, gs_thread, 0) || pthread_join(thread, &result) || result) return 8;
    __asm__ volatile("mov %%gs:8, %0" : "=r"(got));
    if (got != 22) return 9;
    int child = fork();
    if (!child) {
        __asm__ volatile("xor %%eax, %%eax; mov %%gs:(%%rax), %%rdx" : "=d"(got) :: "rax", "cc");
        _exit(got == 11 ? 0 : 1);
    }
    int status = -1;
    if (child < 0 || waitpid(child, &status, 0) != child || status) return 10;
    return syscall(158, 0x1001, old) ? 11 : 0;
}
static void *sibling_mutex;
static int sibling_ready, sibling_release;
static void *sibling_stack(void *ignored) {
    (void)ignored;
    unsigned long mutex[5] = {0};
    int recursive = 1;
    pthread_mutex_init(mutex, &recursive);
    pthread_mutex_lock(mutex);
    sibling_mutex = mutex;
    __atomic_store_n(&sibling_ready, 1, __ATOMIC_RELEASE);
    while (!__atomic_load_n(&sibling_release, __ATOMIC_ACQUIRE)) usleep(1000);
    pthread_mutex_unlock(mutex);
    pthread_mutex_destroy(mutex);
    return 0;
}
int browser_fork_stack_probe(void) {
    unsigned long thread;
    sibling_ready = sibling_release = 0;
    if (pthread_create(&thread, 0, sibling_stack, 0)) return 1;
    while (!__atomic_load_n(&sibling_ready, __ATOMIC_ACQUIRE)) usleep(1000);
    int child = fork();
    if (!child) {
        // The absent sibling still owns its private mutex in the child image.
        // Reading/trying that stack object must neither fault nor unlock it.
        _exit(pthread_mutex_trylock(sibling_mutex) == 16 ? 0 : 2);
    }
    __atomic_store_n(&sibling_release, 1, __ATOMIC_RELEASE);
    pthread_join(thread, 0);
    if (child < 0) return 3;
    int status = -1;
    return waitpid(child, &status, 0) == child && status == 0 ? 0 : 4;
}

typedef int (*UnwindCallback)(void *, void *);
typedef int (*BacktraceFunction)(UnwindCallback, void *);
static int count_frame(void *context, void *count) {
    (void)context;
    ++*(int *)count;
    return 0;
}
__attribute__((noinline)) static int unwind_inner(BacktraceFunction trace, int *count) {
    volatile int result = trace(count_frame, count);
    return result;
}
__attribute__((noinline)) int browser_unwind_probe(BacktraceFunction trace) {
    int count = 0;
    int result = unwind_inner(trace, &count);
    return result == 5 && count >= 3 ? 0 : 100 * result + count;
}

// The guard can be reset after fork, including through a direct TCB pointer.
// Every instruction form must see the same bytes. Restore before returning so
// the caller's own stack protector remains valid, even on a failing runtime.
__attribute__((no_stack_protector)) int browser_guard_probe(void) {
    unsigned long tp, initial, from_fs, from_pointer, after_restore;
    __asm__ volatile("mov %%fs:0, %0" : "=r"(tp));
    __asm__ volatile("mov %%fs:0x28, %0" : "=r"(initial));
    volatile unsigned long *guard = (volatile unsigned long *)(tp + 0x28);
    unsigned long original = *guard, changed = initial ^ 0x1234567800UL;
    __asm__ volatile("mov %0, %%fs:0x28" :: "r"(changed) : "memory");
    from_pointer = *guard;
    __asm__ volatile("mov %%fs:0x28, %0" : "=r"(from_fs));
    *guard = original;
    __asm__ volatile("mov %%fs:0x28, %0" : "=r"(after_restore));
    __asm__ volatile("mov %0, %%fs:0x28" :: "r"(initial) : "memory");
    return initial != original || from_pointer != changed || from_fs != changed
        || after_restore != original;
}

// Chromium uses these exact invalid-flag probes before enabling Mojo's fast
// channel. Exercise the raw instruction path as well as libc's public entry.
int browser_kernel_probe(void) {
    long result;
    const char *name = "";
    __asm__ volatile("syscall" : "=a"(result) : "a"(319L), "D"(name), "S"(~0L)
                     : "rcx", "r11", "memory");
    if (result != -22) return 1;
    __asm__ volatile("syscall" : "=a"(result) : "a"(290L), "D"(0L), "S"(~0L)
                     : "rcx", "r11", "memory");
    return result == -22 ? 0 : 2;
}
typedef union { long double value; struct { unsigned long mantissa; unsigned short exponent; } bits; } Extended;
static long double call(long double x, long double y) {
    long double (*volatile function)(long double,long double) = powl;
    return function(x,y);
}
static __attribute__((noinline)) int power_cases(void) {
    if (call(2,10)!=1024 || call(-2,3)!=-8 || call(-2,4)!=16) return 1;
    if (call(2,-3)!=0.125L || call(0,0)!=1) return 2;
    long double precise = 1.0L + 0x1p-60L;
    if (call(precise,2) != 1.0L + 0x1p-59L) return 3;
    if (call(0x1p+4000L,2)!=0x1p+8000L) return 4;
    if (call(2,-12000)!=0x1p-12000L) return 5;
    long double root=call(2,1.5L), expected=sqrtl(8);
    if (root < expected*(1-0x1p-60L) || root > expected*(1+0x1p-60L)) return 6;
    Extended r={.value=call(-0.0L,3)};
    if (r.bits.exponent!=0x8000 || r.bits.mantissa!=0) return 7;
    r.value=call(-0.0L,-3);
    if (r.bits.exponent!=0xffff || r.bits.mantissa!=0x8000000000000000UL) return 8;
    r.value=call(-2,0.5L);
    if (r.value==r.value) return 9;
    Extended infinity={.bits={0x8000000000000000UL,0x7fff}};
    if (call(-1,infinity.value)!=1 || call(2,-infinity.value)!=0) return 10;
    if (call(infinity.value,-2)!=0 || call(1,r.value)!=1 || call(r.value,0)!=1) return 11;
    r.value=call(2,20000);
    if (r.bits.exponent!=0x7fff || r.bits.mantissa!=0x8000000000000000UL) return 12;
    if (call(2,-20000)!=0) return 13;
    // Repeated calls must leave exactly one x87 result each time.
    for (int i=0;i<1000;i++) if (call(2,10)!=1024) return 14;
    return 0;
}
int browser_power_probe(void) {
    // The Windows/Python caller may use 53-bit x87 intermediates. Evaluate the
    // reference expressions with Linux's extended precision, then restore it.
    unsigned short saved,extended,after;
    __asm__ volatile("fnstcw %0" : "=m"(saved));
    extended=(saved & ~0x0300) | 0x0300;
    __asm__ volatile("fldcw %0" : : "m"(extended) : "memory");
    int result=power_cases();
    __asm__ volatile("fnstcw %0" : "=m"(after));
    __asm__ volatile("fldcw %0" : : "m"(saved) : "memory");
    return after!=extended ? 15 : result;
}
