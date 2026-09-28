/* Keep sigsetjmp/siglongjmp entirely within C, including signal stacks. */
typedef unsigned long word;
typedef struct { word registers[8]; int saved; int pad; word mask[16]; } jump_buf;
typedef struct { void (*handler)(int, void *, void *); word mask[16]; int flags; void *restorer; } action;
typedef struct { void *base; int flags; word size; } signal_stack;
extern int __sigsetjmp(jump_buf *, int) __attribute__((returns_twice));
extern void __longjmp_chk(jump_buf *, int) __attribute__((noreturn));
extern int sigaction(int, const action *, action *);
extern int sigaltstack(const signal_stack *, signal_stack *);
extern int fork(void);
extern int printf(const char *, ...);
extern int *__errno_location(void);
extern int waitpid(int, int *, int);
extern void _exit(int) __attribute__((noreturn));
static jump_buf saved;
static unsigned char alternate[65536];
static volatile int observed;
static int mode;

static void handler(int number, void *info, void *context) {
    (void)context;
    int *fields = info;
    if (fields[0] != number || (number == 11 ? fields[2] != 128 : number != 4 || fields[2] != 1))
        __longjmp_chk(&saved, 2);
    if (number == 4) __longjmp_chk(&saved, 1);
    ++observed;
    if (mode & 4) {
        mode &= ~4;
        int child = fork(), status = -1;
        if (child == 0) __longjmp_chk(&saved, 3);
        int waited;
        do { waited = waitpid(child, &status, 0); } while (waited < 0 && *__errno_location() == 4);
        if (child < 0 || waited != child || status != 0) {
            printf("SIGNAL_FORK_FAILED child=%d status=%d errno=%d\n", child, status, *__errno_location());
            __longjmp_chk(&saved, 2);
        }
    }
    if (mode & 2) __asm__ volatile("ud2");
    __longjmp_chk(&saved, 1);
}

int probe(int requested) {
    mode = requested;
    int use_alternate = requested & 1;
    action previous, previous_ill, selected = {0};
    selected.handler = handler;
    selected.flags = 4 | (use_alternate ? 0x08000000 : 0);
    signal_stack before, stack = {alternate, 0, sizeof(alternate)};
    if (sigaltstack(use_alternate ? &stack : 0, &before) != 0) return 1;
    if (sigaction(11, &selected, &previous) != 0) return 2;
    if (sigaction(4, &selected, &previous_ill) != 0) return 2;
    observed = 0;
    for (int index = 0; index < 4; ++index) {
        int value = __sigsetjmp(&saved, 1);
        if (value == 3) _exit(0);
        if (value == 0) {
            /* Same catchable user-mode #GP as the VMware port probe in lscpu. */
            unsigned int result;
            __asm__ volatile("inl %%dx, %%eax" : "=a"(result) : "d"(0x5658));
            return 3;
        }
        if (value != 1 || observed != index + 1) return 4;
        signal_stack current;
        if (sigaltstack(0, &current) != 0 || (current.flags & 1)) return 5;
    }
    if (sigaction(11, &previous, 0) || sigaction(4, &previous_ill, 0) || sigaltstack(&before, 0)) return 6;
    return 0;
}
