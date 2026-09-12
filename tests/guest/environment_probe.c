/* Native libc must preserve guest-owned environ bindings across fresh DLL loads. */
typedef unsigned long size_t;
extern char **environ;
extern char *getenv(const char *);
extern int setenv(const char *,const char *,int),unsetenv(const char *),putenv(char *),clearenv(void);
extern int fork(void),waitpid(int,int *,int),strcmp(const char *,const char *);
extern long write(int,const void *,size_t);
extern void _exit(int) __attribute__((noreturn));
static void require(int ok,const char *message) {
    if(ok)return;size_t n=0;while(message[n])++n;write(2,message,n);write(2,"\n",1);_exit(91);
}
static void value(const char *name,const char *expected) {
    char *actual=getenv(name);require(actual && !strcmp(actual,expected),name);
}
static void wait_for(int child) {
    int status=0;require(child>0 && waitpid(child,&status,0)==child && status==0,"environment child failed");
}
__attribute__((used,noinline)) static void run(void) {
    require(setenv("INHERITED","parent",1)==0,"set parent value");
    char mutable[]="BORROWED=one";
    require(putenv(mutable)==0,"putenv borrows buffer");
    mutable[9]='t';mutable[10]='w';mutable[11]='o';
    require(setenv("SECOND","kept",1)==0,"grow after putenv");value("BORROWED","two");
    int child=fork();require(child>=0,"first fork");
    if(child==0) {
        value("INHERITED","parent");value("BORROWED","two");
        require(setenv("INHERITED","child",1)==0,"mutate inherited owned vector");
        char *replacement[]={"DIRECT=assigned",0};environ=replacement;
        value("DIRECT","assigned");
        require(setenv("AFTER","mutation",1)==0,"setenv respects direct assignment");
        require(!getenv("INHERITED"),"old vector must not resurface");value("DIRECT","assigned");
        int grandchild=fork();require(grandchild>=0,"second fork");
        if(grandchild==0) {
            value("DIRECT","assigned");value("AFTER","mutation");
            require(unsetenv("DIRECT")==0 && !getenv("DIRECT"),"unset restored state");
            require(clearenv()==0 && !environ && !getenv("AFTER"),"clear restored state");
            require(setenv("FRESH","usable",1)==0,"setenv after clearenv");value("FRESH","usable");
            _exit(0);
        }
        wait_for(grandchild);value("DIRECT","assigned");_exit(0);
    }
    wait_for(child);value("INHERITED","parent");value("BORROWED","two");
    write(1,"ENVIRONMENT_FORK_OK\n",20);_exit(0);
}
__attribute__((naked)) void _start(void) { __asm__("call run\nud2"); }
