/* Retain a mixed batch so allocation measures fresh storage, then validate and
 * release every object. Timings include real first/last-byte page access. */
typedef unsigned long size_t;
extern void *malloc(size_t);
extern void free(void *);
struct timespec { long sec, nsec; };
extern int clock_gettime(int, struct timespec *);
struct mallinfo2 { size_t arena, ordblks, smblks, hblks, hblkhd, usmblks,
    fsmblks, uordblks, fordblks, keepcost; };
extern struct mallinfo2 mallinfo2(void);
static double now(void) {
    struct timespec t; clock_gettime(1, &t);
    return t.sec * 1000.0 + t.nsec / 1000000.0;
}
int probe(double *times, size_t *stats) {
    static const size_t sizes[] = {16, 32, 64, 96, 128, 192, 256, 384,
        512, 768, 1024, 1536, 2048, 3072, 3584, 4096};
    const unsigned count = 32768;
    unsigned char **blocks = malloc(count * sizeof(*blocks));
    if (!blocks) return 1;
    struct mallinfo2 before = mallinfo2();
    double start = now();
    unsigned made = 0;
    for (; made < count; ++made) {
        size_t size = sizes[made % 16];
        blocks[made] = malloc(size);
        if (!blocks[made]) break;
        blocks[made][0] = (unsigned char)made;
        blocks[made][size - 1] = (unsigned char)(made >> 8);
    }
    times[0] = now() - start;
    struct mallinfo2 live = mallinfo2();
    int error = made != count;
    start = now();
    while (made) {
        --made;
        size_t size = sizes[made % 16];
        error |= blocks[made][0] != (unsigned char)made;
        error |= blocks[made][size - 1] != (unsigned char)(made >> 8);
        free(blocks[made]);
    }
    times[1] = now() - start;
    struct mallinfo2 after = mallinfo2();
    stats[0] = live.arena - before.arena;
    stats[1] = live.uordblks - before.uordblks;
    stats[2] = after.uordblks - before.uordblks;
    stats[3] = after.fordblks - before.fordblks;
    free(blocks);
    return error;
}
