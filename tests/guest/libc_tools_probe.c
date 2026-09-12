/* Linux x86-64 ABI regression: real ELF callbacks, service data and temp files. */
typedef unsigned long size_t;
typedef struct chunk { char *limit; struct chunk *prev; char contents[4]; } chunk;
typedef struct obstack {
    long chunk_size;
    chunk *chunk;
    char *object_base, *next_free, *chunk_limit;
    void *temp;
    int alignment_mask;
    void *chunkfun, *freefun, *extra_arg;
    unsigned flags;
} obstack;
struct servent { char *name; char **aliases; int port; char *protocol; };
extern int _obstack_begin(obstack *, int, int, void *(*)(long), void (*)(void *));
extern int _obstack_begin_1(obstack *, int, int, void *(*)(void *, long), void (*)(void *, void *), void *);
extern void _obstack_newchunk(obstack *, int), obstack_free(obstack *, void *);
extern int _obstack_memory_used(obstack *);
extern void *malloc(size_t);
extern void free(void *);
extern struct servent *getservent(void), *getservbyname(const char *, const char *), *getservbyport(int, const char *);
extern void setservent(int), endservent(void);
extern int mkostemps(char *, int, int), mkstemps(char *, int), mkstemp(char *);
extern int open(const char *, int, ...), close(int), unlink(const char *), fcntl(int, int, ...);
extern long write(int, const void *, size_t), read(int, void *, size_t), lseek(int, long, int);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));
typedef struct mbstate { int count; unsigned value; } mbstate;
typedef struct division { long quotient, remainder; } division;
extern size_t __mbrlen(const char *, size_t, mbstate *), mbrtowc(int *, const char *, size_t, mbstate *);
extern int *wcsdup(const int *);
extern division imaxdiv(long, long);
extern int dn_skipname(const unsigned char *, const unsigned char *), ns_name_skip(const unsigned char **, const unsigned char *);
extern void *opendir(const char *), *readdir(void *);
extern int closedir(void *), mkdir(const char *, unsigned), rename(const char *, const char *), rmdir(const char *);
extern int scandir(const char *, void ***, void *, void *);
extern size_t *__libc_stack_end;
extern char *dngettext(const char *, const char *, const char *, size_t);
static size_t length(const char *s) { size_t n=0; while(s[n]) ++n; return n; }
static int equal(const char *a, const char *b) { while(*a && *a==*b) { ++a; ++b; } return *a==*b; }
static void require(int value, const char *error) {
    if(!value) { write(2,error,length(error));write(2,"\n",1);_exit(91); }
}
static int live;
static void *allocate(long size) { void *p=malloc(size);require(p!=0,"allocate");++live;return p; }
static void release(void *p) { --live;free(p); }
static void *allocate_context(void *arg,long size) { require(arg==&live,"allocation context");return allocate(size); }
static void release_context(void *arg,void *p) { require(arg==&live,"release context");release(p); }
static void object_stacks(void) {
    _Static_assert(sizeof(obstack)==88,"obstack size");
    _Static_assert(__builtin_offsetof(obstack,alignment_mask)==48,"alignment mask");
    obstack stack;
    require(_obstack_begin_1(&stack,64,16,allocate_context,release_context,&live),"obstack begin with argument");
    char *first=stack.object_base;
    first[0]='a';stack.object_base=first+16;stack.next_free=stack.object_base;
    _obstack_newchunk(&stack,256);
    require(live==2 && first[0]=='a',"finished object lifetime");
    stack.flags|=2; /* public finish macro preserves an empty object */
    _obstack_newchunk(&stack,1024);
    require(live==3,"empty object chunk lifetime");
    require(_obstack_memory_used(&stack)>1024,"obstack memory accounting");
    obstack_free(&stack,0);
    require(live==0,"obstack full chain release");
    require(_obstack_begin(&stack,0,0,allocate,release),"obstack begin without argument");
    require(((size_t)stack.object_base&15)==0,"default SysV alignment");
    *stack.next_free++='x';
    _obstack_newchunk(&stack,10000);
    require(live==1 && stack.object_base[0]=='x',"unfinished object relocation");
    obstack_free(&stack,0);require(live==0,"plain callback release");
}
static void temporary_files(void) {
    char pattern[]="/tmp/libc-tools-XXXXXX.pkg";
    int fd=mkostemps(pattern,4,0x80000|0x400);
    require(fd>=0 && equal(pattern+length(pattern)-4,".pkg"),"temporary suffix");
    require((fcntl(fd,1)&1)!=0 && (fcntl(fd,3)&0x400)!=0,"temporary flags");
    require(write(fd,"one",3)==3 && lseek(fd,0,0)==0 && write(fd,"two",3)==3,"append writes");
    require(lseek(fd,0,0)==0,"temporary rewind");char data[7]={0};
    require(read(fd,data,6)==6 && equal(data,"onetwo"),"append semantics");
    require(close(fd)==0 && unlink(pattern)==0,"temporary cleanup");
    char invalid[]="/tmp/not-a-template";
    require(mkstemp(invalid)==-1 && *__errno_location()==22 && equal(invalid,"/tmp/not-a-template"),"plain validation");
    char suffix[]="/tmp/XXXXXX.pkg";
    require(mkstemps(suffix,-1)==-1 && *__errno_location()==22 && equal(suffix,"/tmp/XXXXXX.pkg"),"suffix validation");
}
static void services(void) {
    setservent(1);
    struct servent *entry=getservent();
    require(entry && equal(entry->name,"alpha") && equal(entry->aliases[0],"alpha-alias") && entry->aliases[1]==0,"first service");
    require(entry->port==((43210>>8)|((43210&255)<<8)),"network byte order");
    entry=getservent();require(entry && equal(entry->protocol,"sctp"),"service protocol");
    *__errno_location()=123;require(getservent()==0 && *__errno_location()==123,"service EOF");
    setservent(0);entry=getservent();require(entry && equal(entry->name,"alpha"),"service rewind");
    entry=getservbyname("alpha-alias",0);require(entry && equal(entry->name,"alpha"),"service alias");
    entry=getservbyport((43211>>8)|((43211&255)<<8),"sctp");require(entry && equal(entry->name,"beta"),"service port lookup");
    endservent();int fd=open("/etc/services",1|512);require(fd>=0,"open service fixture");close(fd);
    require(getservent()==0 && getservbyname("http","tcp")==0,"empty guest services authoritative");
    endservent();
}
static void shell_strings(void) {
    mbstate state={0};int wide=0;
    require(__mbrlen("A",1,&state)==1 && state.count==0,"C locale character length");
    require(mbrtowc(&wide,"B",1,&state)==1 && wide=='B' && state.count==0,"C locale wide conversion");
    require(__mbrlen("\xc2",1,0)==(size_t)-1 && *__errno_location()==84,"C locale rejects non-ASCII");
    require(mbrtowc(&wide,"A",1,0)==1 && wide=='A',"independent implicit states");
    require(mbrtowc(&wide,"\xed\xa0",2,&state)==(size_t)-1 && *__errno_location()==84,"C locale invalid multibyte input");
    int original[]={0x41,0x20ac,0x1f642,0};int *copy=wcsdup(original);
    require(copy && copy!=original && copy[0]==original[0] && copy[1]==original[1] && copy[2]==original[2] && copy[3]==0,"wide string copy");
    copy[0]=0;require(original[0]==0x41,"wide copy independence");free(copy);
    division result=imaxdiv(-29,6);require(result.quotient==-4 && result.remainder==-5,"intmax division ABI");
}
static void resolver_names(void) {
    const unsigned char plain[]={3,'w','w','w',0,255};
    const unsigned char compressed[]={1,'a',192,255,255};
    require(dn_skipname(plain,plain+sizeof(plain))==5,"plain DNS name consumption");
    require(dn_skipname(compressed,compressed+sizeof(compressed))==4,"DNS pointer consumption");
    for(size_t n=0;n<4;++n) {
        const unsigned char *cursor=compressed;
        require(ns_name_skip(&cursor,compressed+n)==-1 && cursor==compressed && *__errno_location()==90,"truncated DNS preserves cursor");
    }
    const unsigned char invalid[]={64,0};
    require(dn_skipname(invalid,invalid+2)==-1,"reserved DNS label type");
    const unsigned char *cursor=plain;
    require(ns_name_skip(&cursor,plain+sizeof(plain))==0 && cursor==plain+5,"DNS cursor advancement");
}
static void directories(void) {
    void **list=(void **)123;
    require(scandir("/etc/missing-config/directory",&list,0,0)==-1 && *__errno_location()==2 && list==(void **)123,"scandir missing nested directory errno");
    require(opendir("/missing-libc-tools-directory")==0 && *__errno_location()==2,"missing directory errno");
    require(opendir("/etc/services")==0 && *__errno_location()==20,"regular file directory errno");
    void *stream=opendir("/proc/self/ns");require(stream && readdir(stream),"synthetic directory stream");closedir(stream);
    require(mkdir("/tmp/pinned-directory",0700)==0,"create directory fixture");
    stream=opendir("/tmp/pinned-directory");require(stream!=0,"native directory stream");
    require(rename("/tmp/pinned-directory","/tmp/moved-directory")==0 && readdir(stream),"open directory survives rename");
    require(closedir(stream)==0 && rmdir("/tmp/moved-directory")==0,"directory cleanup");
}
__attribute__((noreturn)) void probe_start(void) {
    require(__libc_stack_end && __libc_stack_end[0]==1 && equal(((char **)__libc_stack_end)[1],"/probe"),"initial guest stack publication");
    require(equal(dngettext("test","item","items",0),"items") && equal(dngettext("test","item","items",1),"item") && equal(dngettext("test","item","items",3),"items"),"plural fallback");
    object_stacks();temporary_files();services();shell_strings();resolver_names();directories();
    write(1,"LIBC_TOOLS_OK\n",14);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");
}
