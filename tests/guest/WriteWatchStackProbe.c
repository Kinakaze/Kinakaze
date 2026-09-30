/* Sparse writes, kernel read destinations, and a nested fork of the new child. */
extern int fork(void);
extern int pipe(int *);
extern long write(int, const void *, unsigned long);
extern long read(int, void *, unsigned long);
extern int close(int);
extern int waitpid(int, int *, int);
extern void _exit(int);

#define SIZE (2 * 1024 * 1024)
int probe(void) {
    volatile unsigned char bytes[SIZE];
    bytes[7] = 0x37;
    bytes[1048591] = 0x61;
    bytes[SIZE - 1] = 0x83;
    int descriptors[2], status;
    if (pipe(descriptors)) return 10;
    const unsigned char payload[3] = {0x91, 0x92, 0x93};
    if (write(descriptors[1], payload, 3) != 3) return 11;
    if (read(descriptors[0], (void *)(bytes + 65537), 3) != 3) return 12;
    close(descriptors[0]);
    close(descriptors[1]);
    for (unsigned round = 0; round < 3; ++round) {
        bytes[131079] = 0xa0 + round;
        int child = fork();
        if (child < 0) return 13;
        if (!child) {
            if (bytes[7] != 0x37 || bytes[1048591] != 0x61 || bytes[SIZE - 1] != 0x83
                || bytes[65537] != 0x91 || bytes[65538] != 0x92 || bytes[65539] != 0x93
                || bytes[131079] != 0xa0 + round) _exit(14);
            bytes[1500007] = 0xf5;
            int nested = fork();
            if (nested < 0) _exit(15);
            if (!nested) {
                if (bytes[1500007] != 0xf5 || bytes[7] != 0x37 || bytes[SIZE - 1] != 0x83)
                    _exit(16);
                bytes[7] = 0xee;
                _exit(0);
            }
            if (waitpid(nested, &status, 0) != nested || status || bytes[7] != 0x37) _exit(17);
            bytes[7] = 0xff;
            _exit(0);
        }
        if (waitpid(child, &status, 0) != child || status || bytes[7] != 0x37) return 18;
    }
    return 0;
}
