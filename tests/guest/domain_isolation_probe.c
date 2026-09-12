typedef unsigned long size_t;
extern int open(const char *, int, ...), close(int), usleep(unsigned int);
extern int mount(const char *, const char *, const char *, unsigned long, const void *);
extern int socket(int, int, int), bind(int, const void *, unsigned int);
extern int fork(void), waitpid(int, int *, int), execve(const char *, char *const *, char *const *);
extern long read(int, void *, size_t), write(int, const void *, size_t);
extern void _exit(int) __attribute__((noreturn));
static void require(int ok, const char *why) {
    if (!ok) { size_t n=0; while(why[n]) ++n; write(2,why,n); write(2,"\n",1); _exit(93); }
}
static int contains(const char *s, size_t length, const char *text) {
    for(size_t i=0;i<length;++i) {
        size_t j=0; while(text[j] && i+j<length && s[i+j]==text[j]) ++j;
        if (!text[j]) return 1;
    }
    return 0;
}
static void forwarding(char expected) {
    char value[2]; int fd=open("/proc/sys/net/ipv4/ip_forward",0);
    require(fd>=0 && read(fd,value,2)==2 && value[0]==expected && !close(fd),"manager network state mismatch");
}
static void set_forwarding(void) {
    int fd=open("/proc/sys/net/ipv4/ip_forward",1);
    require(fd>=0 && write(fd,"1\n",2)==2 && !close(fd),"set first domain forwarding");
}
static void exec_check(void) {
    char *args[]={"/probe","check",0}, *env[]={0};
    execve(args[0],args,env); _exit(94);
}
__attribute__((used,noinline,noreturn)) static void probe_start(const size_t *stack) {
    if(stack[0]==2) {
        const char *argument=(const char *)stack[2];
        if(argument[0]=='e') { set_forwarding(); exec_check(); }
        forwarding('1'); write(1,"NETWORK_EXEC_OWNER_OK\n",22); _exit(0);
    }
    char mode; int fd=open("/mode",0);
    require(fd>=0 && read(fd,&mode,1)==1 && !close(fd),"read probe mode");
    if (mode=='a') {
        require(!mount("/source","/target","",4096,0),"bind in first manager domain");
        set_forwarding();
        int child=fork(); require(child>=0,"fork network owner");
        if(!child) {
            forwarding('1');
            exec_check();
        }
        int status=-1;require(waitpid(child,&status,0)==child && status==0,"network state survives fork and exec");
    } else {
        forwarding('0');
        char info[16384]; fd=open("/proc/self/mountinfo",0);
        long n=fd<0 ? -1 : read(fd,info,sizeof(info));
        require(n>0 && !close(fd),"read second domain mountinfo");
        require(!contains(info,(size_t)n," /target "),"foreign manager mount leaked");
    }
    // The same abstract name must be independently bindable in each domain.
    struct { unsigned short family; char path[108]; } address={1,"\0kinakaze-domain-isolation"};
    int sock=socket(1,1,0);
    require(sock>=0 && !bind(sock,&address,28),"foreign manager abstract socket leaked");
    fd=open(mode=='a' ? "/ready-a" : "/ready-b",1|64|512,0600);
    require(fd>=0 && write(fd,"ready\n",6)==6 && !close(fd),"publish readiness");
    for (;;) {
        fd=open("/finish",0);
        if(fd>=0) { close(fd); break; }
        usleep(10000);
    }
    require(!close(sock),"close abstract socket");
    write(1,"DOMAIN_ISOLATION_OK\n",20); _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2"); }
