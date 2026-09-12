typedef unsigned long size_t;
struct limit { unsigned long soft, hard; };
struct pollfd { int fd; short events, revents; };
extern int open(const char *,int,...),close(int),unlink(const char *),ftruncate(int,long);
extern long write(int,const void *,size_t),pwrite(int,const void *,size_t,long);
extern int getrlimit(int,struct limit *),setrlimit(int,const struct limit *);
extern int socketpair(int,int,int,int *),poll(struct pollfd *,unsigned long,int);
extern int fork(void),waitpid(int,int *,int),setuid(unsigned),*__errno_location(void);
extern void *signal(int,void *);
extern void _exit(int) __attribute__((noreturn));
static volatile int signals;
static void handler(int n) { if(n==25) ++signals; }
static void require(int value,const char *message) {
    if(value)return; size_t n=0;while(message[n])++n;write(2,message,n);write(2,"\n",1);_exit(91);
}
__attribute__((used,noinline)) static void run(void) {
    struct limit original,small,nofile;
    require(getrlimit(1,&original)==0,"get file limit");
    signal(25,(void *)handler);
    int fd=open("/tmp/rlimit-probe",0100|01000|2,0600);require(fd>=0,"open regular file");
    small.soft=4;small.hard=original.hard;require(setrlimit(1,&small)==0,"set file limit");
    require(write(fd,"abcdef",6)==4,"write stops at limit");
    require(write(fd,0,0)==0,"zero write at limit");
    require(write(fd,"x",1)==-1 && *__errno_location()==27 && signals==1,"write EFBIG and SIGXFSZ");
    require(pwrite(fd,"y",1,4)==-1 && *__errno_location()==27 && signals==2,"pwrite EFBIG and SIGXFSZ");
    require(ftruncate(fd,5)==-1 && *__errno_location()==27 && signals==3,"truncate EFBIG and SIGXFSZ");
    int child=fork();require(child>=0,"fork under file limit");
    if(child==0) {
        struct limit inherited;require(getrlimit(1,&inherited)==0 && inherited.soft==4,"fork inherits quota");
        _exit(0);
    }
    int status=0;require(waitpid(child,&status,0)==child && status==0,"quota child exits");
    require(setrlimit(1,&original)==0,"restore file limit");
    close(fd);unlink("/tmp/rlimit-probe");
    int pair[2];require(socketpair(1,1,0,pair)==0,"unix socketpair");
    require(write(pair[0],"ready",5)==5,"socket is not subject to file-size limit");
    require(getrlimit(7,&nofile)==0,"get fd limit");small.soft=1;small.hard=nofile.hard;
    require(setrlimit(7,&small)==0,"lower fd quota below existing fds");
    struct pollfd watched={pair[1],1,0};
    require(poll(&watched,1,1000)==1 && (watched.revents&1),"poll works without spare descriptor");
    require(open("/tmp/no-fd",0100|2,0600)==-1 && *__errno_location()==24,"fd quota still enforced");
    require(setrlimit(7,&nofile)==0,"restore fd limit");close(pair[0]);close(pair[1]);unlink("/tmp/no-fd");
    child=fork();require(child>=0,"fork quota test child");
    if(child==0) {
        small.soft=small.hard=0;require(setrlimit(6,&small)==0,"set zero process quota");
        require(setuid(101)==0,"drop test child uid");
        require(fork()==-1 && *__errno_location()==11,"zero process quota rejects creation");
        _exit(0);
    }
    require(waitpid(child,&status,0)==child && status==0,"process quota child exits");
    write(1,"RLIMIT_POLL_OK\n",15);_exit(0);
}
__attribute__((naked)) void _start(void) { __asm__("call run\nud2"); }
