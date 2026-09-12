/* Real Linux ABI, optimized ELF CFI and process-state transfer regression. */
typedef unsigned long size_t;
extern long write(int, const void *, size_t), syscall(long, ...);
extern int *__errno_location(void);
extern int fork(void), waitpid(int, int *, int), close(int), unlink(const char *);
extern int setfsuid(unsigned), setfsgid(unsigned), setresuid(unsigned,unsigned,unsigned), seteuid(unsigned);
extern unsigned geteuid(void), getegid(void);
extern int open(const char *, int, ...), fstat(int, void *), futimes(int, const void *);
extern int getdomainname(char *, size_t), setdomainname(const char *, size_t);
extern unsigned inet_addr(const char *);
extern int innetgr(const char *, const char *, const char *, const char *);
extern int backtrace(void **, int);
extern char **backtrace_symbols(void *const *, int);
extern void free(void *), _exit(int) __attribute__((noreturn));
extern void *malloc(size_t), *mmap(void *,size_t,int,int,int,long);
extern int madvise(void *,size_t,int), mprotect(void *,size_t,int), munmap(void *,size_t);
struct mallinfo2 { size_t arena,ordblks,smblks,hblks,hblkhd,usmblks,fsmblks,used,available,tail; };
extern struct mallinfo2 mallinfo2(void);
struct glob_result { size_t count; char **paths; size_t offset; int flags; void *callbacks[5]; };
extern int glob64(const char *,int,void *,struct glob_result *);
extern void globfree64(struct glob_result *);
extern int asprintf(char **,const char *,...), ffs(int);
struct obstack { long chunk_size; void *chunk; char *base,*next,*limit; void *temp; int mask; void *alloc,*release,*arg; unsigned flags; };
extern int _obstack_begin(struct obstack *,int,int,void *(*)(long),void (*)(void *));
extern int obstack_printf(struct obstack *,const char *,...);
extern void obstack_free(struct obstack *,void *);
static void *chunk_allocate(long size) { return malloc((size_t)size); }
struct times { long sec, sub; };
struct stat64 { unsigned long dev, ino, nlink; unsigned mode, uid, gid, pad; unsigned long rdev; long size, blksize, blocks; struct times atime, mtime, ctime; long reserved[3]; };
static void require(int value, const char *message) {
    if(value) return;
    size_t n=0; while(message[n]) ++n;
    write(2,message,n); write(2,"\n",1); _exit(91);
}
static int equal(const char *a,const char *b) { while(*a && *a==*b) {++a;++b;} return *a==*b; }
static volatile int unwind_depth;
__attribute__((noinline)) int unwind_leaf(void) {
    void *frames[16];
    int n=backtrace(frames,16);
    require(n>=3 && n<=16,"DWARF backtrace through optimized ELF frames");
    char **symbols=backtrace_symbols(frames,n);
    require(symbols && symbols[0] && symbols[1],"backtrace symbol allocation");
    int found=0;
    for(int i=0;i<n;++i) { const char *p=symbols[i]; for(;*p;++p) if(p[0]=='u'&&p[1]=='n'&&p[2]=='w'&&p[3]=='i'&&p[4]=='n'&&p[5]=='d') {found=1;break;} }
    require(found,"backtrace must symbolize real guest functions");
    free(symbols); return n;
}
__attribute__((noinline)) int unwind_middle(void) { int n=unwind_leaf(); unwind_depth=n; return n+1; }
__attribute__((noinline)) int unwind_outer(void) { int n=unwind_middle(); unwind_depth=n; return n+1; }
static void identity(void) {
    require(setfsuid(1001)==0 && setfsgid(1002)==0,"root filesystem identity change");
    *__errno_location()=37;
    require(setfsuid(~0u)==1001 && setfsgid(~0u)==1002 && *__errno_location()==37,"filesystem query preserves errno");
    require(geteuid()==0 && getegid()==0,"fsids do not change effective IDs");
    int child=fork(); require(child>=0,"fork filesystem identity");
    if(!child) {
        require(setfsuid(~0u)==1001 && setfsgid(~0u)==1002,"child inherits fsids");
        require(unwind_outer()>3,"child rebuilds unwind services");
        int fd=open("/tmp/fs-owned",0102,0600); require(fd>=0,"create with fsids");
        struct stat64 st;
        require(!fstat(fd,&st) && st.uid==1001 && st.gid==1002,"inode ownership uses fsids");
        require(!unlink("/tmp/fs-owned"),"unlink test file");
        struct times tv[2]={{1234567890,123456},{1234567891,654321}};
        require(!futimes(fd,tv) && !fstat(fd,&st) && st.mtime.sec==1234567891 && st.mtime.sub==654321000,"futimes timestamps unlinked inode");
        tv[0].sub=1000000;
        require(futimes(fd,tv)==-1 && *__errno_location()==22,"futimes rejects invalid microseconds");
        close(fd); _exit(0);
    }
    int status=-1; require(waitpid(child,&status,0)==child && status==0,"fork child result");
    require(setfsuid(~0u)==1001,"child leaves parent fsuid intact");
    require(!setresuid(0,1000,0) && setfsuid(~0u)==1000,"setresuid updates fsuid");
    *__errno_location()=37;
    require(setfsuid(2000)==1000 && setfsuid(~0u)==1000 && *__errno_location()==37,"unprivileged fsuid denial");
    require(setfsuid(0)==1000 && setfsuid(~0u)==0,"real UID authorizes fsuid");
    require(!seteuid(0),"restore effective UID");
    require(syscall(122,1003)==0 && syscall(122,~0u)==1003,"raw setfsuid syscall");
    setfsuid(0); setfsgid(0);
}
void run_probe(void) {
    require(unwind_outer()>3,"parent unwind");
    identity();
    char domain[65]; require(!setdomainname("fixture",7) && !getdomainname(domain,sizeof(domain)) && equal(domain,"fixture"),"UTS domain roundtrip");
    require(getdomainname(domain,7)==-1 && *__errno_location()==22,"domain truncation rejects");
    require(inet_addr("127.1")==0x0100007f && inet_addr("0x7f000001")==0x0100007f && inet_addr("invalid")==~0u,"historical IPv4 formats");
    require(innetgr("outer","host","alice","fixture")==1,"nested netgroup match");
    require(!innetgr("outer","host","bob","fixture") && !innetgr("cycle","x","x","x"),"netgroup mismatch and cycle");
    require(write(1,0,0)==0 && write(-1,0,0)==-1 && *__errno_location()==9,"zero-byte IO validates FD without dereferencing NULL");
    void *page=mmap(0,4096,0,0x22,-1,0);
    require(page!=(void *)-1 && !madvise(page,4096,4) && !mprotect(page,4096,3),"discard untouched PROT_NONE reservation");
    require(!*(unsigned long *)page && !munmap(page,4096),"discard retains zero-fill and address");
    void *allocation=malloc(3*1024*1024); require(allocation!=0,"statistics allocation");
    struct mallinfo2 live=mallinfo2(); free(allocation);
    struct mallinfo2 released=mallinfo2();
    require(live.used>=3*1024*1024 && released.used<live.used && released.available>live.available && live.used+live.available==live.arena,"mallinfo2 reads real allocator state");
    struct glob_result matches={0}; matches.offset=2;
    require(!glob64("/tmp/glob/*.txt",8,0,&matches) && matches.count==2 && !matches.paths[0] && !matches.paths[1] && equal(matches.paths[2],"/tmp/glob/a.txt") && equal(matches.paths[3],"/tmp/glob/b.txt"),"glob expands sorts and preserves offset");
    require(!glob64("/tmp/glob/c.dat",8|32,0,&matches) && matches.count==3 && equal(matches.paths[4],"/tmp/glob/c.dat"),"glob append preserves old entries");
    globfree64(&matches);
    require(glob64("/tmp/glob/absent*",0,0,&matches)==3,"glob reports actual no-match");
    char *formatted=0;
    require(asprintf(&formatted,"%s:%d %.1f","value",42,1.5)==12 && equal(formatted,"value:42 1.5"),"asprintf SysV integer and SSE varargs");
    free(formatted);
    struct obstack objects={0};
    require(_obstack_begin(&objects,64,0,chunk_allocate,free),"obstack formatting setup");
    require(obstack_printf(&objects,"%s:%d","value",42)==8 && objects.next-objects.base==8,"obstack printf grows current object");
    obstack_free(&objects,0);
    require(ffs(0)==0 && ffs(0x80000000)==32 && ffs(12)==3,"ffs bit positions");
    write(1,"LIBC_GAP_OK\n",12); _exit(0);
}
__attribute__((naked)) void _start(void) { __asm__("call run_probe\n ud2"); }
