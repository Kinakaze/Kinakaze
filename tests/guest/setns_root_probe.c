typedef unsigned long size_t;
extern int open(const char *, int, ...), close(int), chdir(const char *), fchdir(int), pipe(int *);
extern int mount(const char *, const char *, const char *, unsigned long, const void *);
extern int unshare(int), setns(int,int), pivot_root(const char *, const char *), umount2(const char *, int);
extern int fork(void), waitpid(int,int *,int), snprintf(char *,size_t,const char *,...);
extern long read(int,void *,size_t),write(int,const void *,size_t);
extern char *getcwd(char *,size_t);
extern void _exit(int) __attribute__((noreturn));
static void require(int ok,const char *why) {
    if(!ok) { size_t n=0;while(why[n]) ++n;write(2,why,n);write(2,"\n",1);_exit(93); }
}
static void check_root(void) {
    char cwd[512],value[5];
    require(getcwd(cwd,sizeof(cwd)) && cwd[0]=='/' && !cwd[1],"setns must reset cwd to namespace root");
    int fd=open("/value",0);
    require(fd>=0 && read(fd,value,5)==5 && value[0]=='j' && !close(fd),"setns must select namespace root for file lookup");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    int original=open("/proc/self/ns/mnt",0), ready[2],finish[2];
    require(original>=0 && !pipe(ready) && !pipe(finish),"open namespace and coordination pipes");
    require(!chdir("/outside"),"initial cwd outside target namespace root");
    int child=fork();require(child>=0,"fork namespace owner");
    if(!child) {
        close(ready[0]);close(finish[1]);
        require(!unshare(0x20000),"unshare target mount namespace");
        require(!mount("","/","",(1ul<<18)|16384,0),"make target namespace private");
        require(!mount("/newroot","/newroot","",4096,0),"bind target root");
        int old=open("/",0x200000|0x10000);require(old>=0,"retain old root");
        require(!chdir("/newroot") && !pivot_root(".","."),"pivot target namespace root");
        require(!fchdir(old) && !umount2(".",2) && !chdir("/") && !close(old),"detach target old root");
        check_root();
        char byte;require(write(ready[1],"x",1)==1 && read(finish[0],&byte,1)==1,"keep target namespace alive");
        _exit(0);
    }
    close(ready[1]);close(finish[0]);
    char byte,path[80];require(read(ready[0],&byte,1)==1,"target namespace ready");
    snprintf(path,sizeof(path),"/proc/%d/ns/mnt",child);
    int target=open(path,0);require(target>=0,"open target namespace descriptor");
    require(!setns(target,0x20000),"enter target mount namespace");
    check_root();
    int nested=fork();require(nested>=0,"fork immediately after mount setns");
    if(!nested) { check_root();_exit(0); }
    int status=-1;require(waitpid(nested,&status,0)==nested && status==0,"forked setns root restoration");
    require(!setns(original,0x20000),"return to original mount namespace");
    char cwd[512];require(getcwd(cwd,sizeof(cwd)) && cwd[0]=='/' && !cwd[1],"return resets cwd");
    int outside=open("/outside",0x200000|0x10000);require(outside>=0 && !close(outside),"return restores original root");
    require(write(finish[1],"x",1)==1 && waitpid(child,&status,0)==child && status==0,"target owner exits cleanly");
    close(original);close(target);close(ready[0]);close(finish[1]);
    write(1,"SETNS_ROOT_FORK_OK\n",19);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
