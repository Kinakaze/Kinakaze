typedef unsigned long size_t;
extern void *malloc(size_t);
extern void *realloc(void *, size_t);
extern void free(void *);
extern size_t malloc_usable_size(void *);
struct timespec { long sec, nsec; };
extern int clock_gettime(int, struct timespec *);
static double now(void) {
    struct timespec t; clock_gettime(1, &t);
    return (double)t.sec * 1000 + (double)t.nsec / 1000000;
}
int concurrent_growth(unsigned seed) {
    for (unsigned iteration=0; iteration<300; ++iteration) {
        unsigned char *blocks[8];
        size_t sizes[8];
        for (unsigned i=0; i<8; ++i) {
            sizes[i]=257+((iteration*97+seed*37+i*113)%4096);
            blocks[i]=malloc(sizes[i]);
            if (!blocks[i]) return 10;
            blocks[i][0]=(unsigned char)(seed+i);
            blocks[i][sizes[i]-1]=0xa3;
        }
        for (unsigned round=0; round<3; ++round) {
            for (unsigned i=0; i<8; ++i) {
                size_t next=sizes[i]+1024+round*32768;
                unsigned char *p=realloc(blocks[i], next);
                if (!p || p[0]!=(unsigned char)(seed+i) || p[sizes[i]-1]!=0xa3) return 11;
                p[next-1]=0xa3; blocks[i]=p; sizes[i]=next;
            }
        }
        for (unsigned i=0; i<8; ++i) free(blocks[7-i]);
    }
    return 0;
}
void *grow_for_fork(void) {
    unsigned char *p=malloc(3*1024*1024);
    if (!p) return 0;
    p[0]=0x73;
    unsigned char *grown=realloc(p, 6*1024*1024);
    if (!grown) { free(p); return 0; }
    grown[6*1024*1024-1]=0x92;
    return grown;
}
/* Keep the caller/Python out of the timed allocation sequence. Retaining the
 * final blocks makes repetitions representative of several growing buffers. */
int probe(double *milliseconds, unsigned *moves) {
    const size_t mib = 1024 * 1024;
    unsigned char *retained[7] = {0};
    for (int repeat=0; repeat<7; ++repeat) {
        unsigned char *p=malloc(mib);
        if (!p) return 1;
        for (size_t i=0; i<mib; ++i) p[i]=(unsigned char)(i*13+7);
        double start=now(); moves[repeat]=0;
        for (size_t n=2; n<=32; ++n) {
            unsigned char *grown=realloc(p, n*mib);
            if (!grown || malloc_usable_size(grown)<n*mib) return 2;
            moves[repeat]+=(grown!=p); p=grown;
            if (p[0]!=7 || p[mib-1]!=(unsigned char)((mib-1)*13+7)) return 3;
            p[n*mib-1]=0x5a;
        }
        milliseconds[repeat]=now()-start;
        for (size_t i=0; i<mib; ++i)
            if (p[i]!=(unsigned char)(i*13+7)) return 4;
        for (size_t n=2; n<=32; ++n) if (p[n*mib-1]!=0x5a) return 5;
        if (realloc(p, (size_t)-1)!=0 || p[0]!=7) return 6;
        retained[repeat]=p;
    }
    /* A later allocation prevents in-place growth; its contents must survive. */
    unsigned char *a=malloc(1777), *guard=malloc(2345);
    if (!a || !guard) return 7;
    a[0]=0x31; a[1776]=0x42; guard[0]=0x77; guard[2344]=0x88;
    unsigned char *b=realloc(a, 13*mib);
    if (!b || b[0]!=0x31 || b[1776]!=0x42 || guard[0]!=0x77 || guard[2344]!=0x88) return 8;
    free(b); free(guard);
    for (int i=0; i<7; ++i) free(retained[i]);
    return 0;
}
