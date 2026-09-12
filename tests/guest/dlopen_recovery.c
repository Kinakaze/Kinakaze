static int initialized;
#ifdef BAD_SYMBOL
extern void kinakaze_nonexistent_recovery_symbol(void);
void probe_missing(void) { kinakaze_nonexistent_recovery_symbol(); }
#endif
__attribute__((constructor)) static void init(void) {
#if defined(BAD_SYMBOL) || defined(BAD_NEEDED)
    __builtin_trap(); /* A failed dlopen must never reach this initializer. */
#else
    ++initialized;
#endif
}
__attribute__((destructor)) static void fini(void) {
#if defined(BAD_SYMBOL) || defined(BAD_NEEDED)
    __builtin_trap(); /* Nor may process teardown visit this destructor. */
#endif
}
int probe_value(void) { return initialized == 1 ? 42 : -1; }
