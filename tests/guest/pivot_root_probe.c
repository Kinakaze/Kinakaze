typedef unsigned long size_t;
extern int open(const char *, int, ...), close(int), fchdir(int), chdir(const char *);
extern int mount(const char *, const char *, const char *, unsigned long, const void *);
extern int unshare(int), pivot_root(const char *, const char *), umount2(const char *, int);
extern int fork(void), waitpid(int, int *, int), *__errno_location(void);
extern long read(int, void *, size_t), write(int, const void *, size_t);
extern char *getcwd(char *, size_t);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s) { size_t n=0; while(s[n]) ++n; return n; }
static int equal(const char *a,const char *b) { while(*a && *a==*b) { ++a; ++b; } return *a==*b; }
static void require(int ok,const char *why) {
    if (!ok) {
        write(2,why,length(why));
        int e=*__errno_location(); char digits[16];int n=0;
        do { digits[n++]=(char)('0'+e%10);e/=10; } while(e);
        write(2," errno=",7);while(n) write(2,&digits[--n],1);
        write(2,"\n",1);_exit(93);
    }
}
static void pivot_sequence(void) {
    require(!unshare(0x20000),"unshare mount namespace");
    require(!mount("","/","",(1ul<<18)|16384,0),"make inherited root private");
    require(!mount("/newroot","/newroot","",4096|16384,0),"self bind new root");
    int old=open("/",0x200000|0x10000), next=open("/newroot",0x200000|0x10000);
    require(old>=0 && next>=0,"open root directory references");
    require(!fchdir(next),"fchdir new root");
    require(!pivot_root(".","."),"stacked pivot_root");
    require(!fchdir(old),"fchdir retained old root");
    char cwd[1024];require(getcwd(cwd,sizeof(cwd))!=0,"cwd outside new root");
    write(1,cwd,length(cwd));write(1,"\n",1);
    require(!mount("",".","",(1ul<<19)|16384,0),"make old root recursively slave");
    require(!umount2(".",2),"detach old root");
    require(!chdir("/"),"enter new root");
    require(getcwd(cwd,sizeof(cwd)) && equal(cwd,"/"),"new root cwd is slash");
    int fd=open("/value",0);char content[5];
    require(fd>=0 && read(fd,content,5)==5 && content[0]=='j' && content[4]=='\n',"new root file lookup");
    require(!close(fd) && !close(old) && !close(next),"close retained directory references");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    int child=fork();require(child>=0,"fork with configured namespace base");
    if (!child) { pivot_sequence();_exit(0); }
    int status=-1;require(waitpid(child,&status,0)==child && status==0,"forked pivot status");
    pivot_sequence();
    write(1,"PIVOT_ROOT_OK\n",14);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
