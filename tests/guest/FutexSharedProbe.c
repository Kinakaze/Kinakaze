/* Real guest signal handlers: the outer wait must be gone before reentry. */
typedef unsigned long u64;
extern long syscall(long, ...);
static int *address;
static volatile int calls, nested;
static void handler(int signal) {
    (void)signal;
    ++calls;
    nested = (int)syscall(202L, address, 1, 1, 0L, 0L, 0);
}
int install_handler(int *word, int restart) {
    struct { void (*handler)(int); u64 flags; void *restorer; u64 mask; } action;
    address = word;
    calls = 0;
    nested = -999;
    action.handler = handler;
    action.flags = restart ? 0x10000000UL : 0;
    action.restorer = 0;
    action.mask = 0;
    return (int)syscall(13L, 10, &action, 0L, 8L);
}
int handler_calls(void) { return calls; }
int handler_nested_wake(void) { return nested; }
