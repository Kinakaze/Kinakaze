/* Fork must preserve every byte of sparse/dense stacks, including PROT_NONE. */
extern int fork(void);
extern int waitpid(int, int *, int);
extern int mprotect(void *, unsigned long, int);
extern void _exit(int);

#define SIZE (2 * 1024 * 1024)
static unsigned char expected(unsigned long index, unsigned round) {
    if (round == 1) return (unsigned char)(index * 29 + round);
    return index == 7 || index == 65543 || index == 1048607 || index == SIZE - 1
        ? (unsigned char)(index * 31 + 97 + round) : 0;
}
int probe(void) {
    volatile unsigned char bytes[SIZE];
    for (unsigned round = 0; round < 4; ++round) {
        for (unsigned long i = 0; i < SIZE; ++i) bytes[i] = expected(i, round);
        unsigned long page = ((unsigned long)bytes + 1024 * 1024 + 4095) & ~4095UL;
        if (round == 3 && mprotect((void *)page, 4096, 0)) return 10;
        int child = fork(), status = -1;
        if (child < 0) return 11;
        if (child == 0) {
            if (round == 3 && mprotect((void *)page, 4096, 3)) _exit(12);
            for (unsigned long i = 0; i < SIZE; ++i)
                if (bytes[i] != expected(i, round)) _exit(13);
            bytes[7] ^= 0xff;
            _exit(0);
        }
        if (waitpid(child, &status, 0) != child || status != 0) return 14;
        if (round == 3) {
            /* The parent's protection must have been restored after copying. */
            child = fork();
            if (child < 0) return 15;
            if (child == 0) { (void)*(volatile unsigned char *)page; _exit(16); }
            if (waitpid(child, &status, 0) != child || (status & 127) != 11) return 17;
            if (mprotect((void *)page, 4096, 3)) return 18;
        }
        for (unsigned long i = 0; i < SIZE; ++i)
            if (bytes[i] != expected(i, round)) return 19;
    }
    return 0;
}
