
typedef unsigned long size_t;
extern void *malloc(size_t);
extern void free(void*);
extern size_t malloc_usable_size(void*);
struct timespec { long sec,nsec; };
extern int clock_gettime(int,struct timespec*);
struct mallinfo2 { size_t arena,ordblks,smblks,hblks,hblkhd,usmblks,fsmblks,uordblks,fordblks,keepcost; };
extern struct mallinfo2 mallinfo2(void);
static double now(void) { struct timespec t; clock_gettime(1,&t);return t.sec*1000.0+t.nsec/1000000.0; }
/* Vary the size class without retaining earlier scratch buffers. Account for
 * allocator used/free bytes separately from committed pages and Windows RSS. */
int probe(double *milliseconds, size_t *stats) {
 struct mallinfo2 before=mallinfo2();
 volatile unsigned char *guard=malloc(65537); if (!guard) return 1;
 guard[0]=0x57;guard[65536]=0x68;
 for (int repeat=0;repeat<8;++repeat) {
  double start=now();
  for (int i=0;i<24;++i) {
   size_t size=98304+196608*i;
   volatile unsigned char *p=malloc(size);if(!p || malloc_usable_size((void*)p)<size) return 2;
   for(size_t j=0;j<size;j+=4096) p[j]=(unsigned char)(i+j);
   p[size-1]=0xab;
   for(size_t j=0;j<size;j+=4096) if(p[j]!=(unsigned char)(i+j)) return 3;
   if(p[size-1]!=0xab || guard[0]!=0x57 || guard[65536]!=0x68) return 4;
   free((void*)p);
  }
  milliseconds[repeat]=now()-start;
  if(repeat==0) {
   struct mallinfo2 after=mallinfo2();
   stats[0]=after.arena-before.arena;
   stats[1]=after.uordblks-before.uordblks;
   stats[2]=after.fordblks-before.fordblks;
   stats[3]=after.keepcost;
  }
 }
 free((void*)guard);return 0;
}
