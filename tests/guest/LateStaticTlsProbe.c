/* Initial-exec TLS loaded after Python and its worker threads have started. */
static int marker = 123;
static __thread int initialized = 37;
static __thread int *relocated = &marker;
static __thread unsigned char zeroed[73] __attribute__((aligned(64)));

int tls_check(int expected, int next) {
    if (initialized != expected || relocated != &marker || *relocated != 123)
        return 1;
    if ((unsigned long)zeroed % 64 || zeroed[72] != (unsigned char)(expected - 37))
        return 2;
    initialized = next;
    zeroed[72] = (unsigned char)(next - 37);
    return 0;
}

void *tls_address(void) { return &initialized; }
