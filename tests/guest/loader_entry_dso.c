/* Real ELF DSO alongside the native PE loader; no libc/startup fixture. */
static int initialized;
static __thread int value = 31;
__attribute__((constructor)) static void initialize(void) { initialized += 20; }
int loader_dso_value(void) { return initialized; }
int *loader_dso_tls(void) { return &value; }
