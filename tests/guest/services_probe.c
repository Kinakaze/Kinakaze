typedef unsigned long size_t;
struct servent { char *name; char **aliases; int port; char *protocol; };
struct addrinfo { int flags, family, socktype, protocol; unsigned length; void *address; char *canonical; struct addrinfo *next; };
extern struct servent *getservbyname(const char *, const char *), *getservbyport(int, const char *), *getservent(void);
extern void setservent(int), endservent(void), freeaddrinfo(struct addrinfo *);
extern int getservbyport_r(int, const char *, struct servent *, char *, size_t, struct servent **);
extern int getservbyname_r(const char *, const char *, struct servent *, char *, size_t, struct servent **);
extern int getaddrinfo(const char *, const char *, const struct addrinfo *, struct addrinfo **);
extern int open(const char *, int, ...), close(int), rename(const char *, const char *), fork(void), waitpid(int,int *,int);
extern int *__errno_location(void);
extern long read(int,void *,size_t), write(int,const void *,size_t);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s) { size_t n=0;while(s[n]) ++n;return n; }
static int equal(const char *a,const char *b) { while(*a && *a==*b) { ++a;++b; }return *a==*b; }
static void require(int ok,const char *why) { if(!ok) {write(2,why,length(why));write(2,"\n",1);_exit(92);} }
static int network(int port) { return (port>>8)|((port&255)<<8); }
static void fixture(const char *text) {
    int fd=open("/etc/services",1|64|512,0600);require(fd>=0,"open services fixture");
    require(write(fd,text,length(text))==(long)length(text) && !close(fd),"write services fixture");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    _Static_assert(sizeof(struct servent)==32,"servent ABI");
    *__errno_location()=0;
    require(!getservbyname("http","tcp") && *__errno_location()==2,"missing services must not read host or built-in defaults");
    struct addrinfo hints={.flags=4,.family=2,.socktype=1},*answers=0;
    require(getaddrinfo("127.0.0.1","http",&hints,&answers)==-8 && !answers,"resolver must not obtain host service names");
    fixture("alpha 43210/tcp alias\nbeta 43211/sctp\ngamma 43212/udp\n");
    setservent(1);
    struct servent *first=getservent();
    require(first && equal(first->name,"alpha"),"first enumeration row");
    struct servent *alias=getservbyname("alias","tcp");
    require(alias && alias->port==network(43210),"service alias lookup");
    struct servent *second=getservent();
    require(second && equal(second->name,"beta"),"lookup must not reset enumeration");
    int child=fork();require(child>=0,"fork enumeration cursor");
    if(!child) {
        require(equal(second->name,"beta"),"inherited servent storage");
        struct servent *next=getservent();require(next && equal(next->name,"gamma"),"child inherits buffered enumeration position");
        endservent();_exit(0);
    }
    int status=-1;require(waitpid(child,&status,0)==child && !status,"fork enumeration status");
    struct servent *next=getservent();require(next && equal(next->name,"gamma"),"child cursor does not consume parent buffer");
    require(!getservent(),"enumeration EOF");
    int fd=open("/etc/services",1|1024,0);require(fd>=0,"append after EOF");
    const char *extra="delta 43213/tcp\n";
    require(write(fd,extra,length(extra))==(long)length(extra) && !close(fd),"append services row");
    child=fork();require(child>=0,"fork EOF cursor");
    if(!child) {
        require(!getservent(),"child retains EOF until rewind");
        setservent(0);next=getservent();require(next && equal(next->name,"alpha"),"child rewind clears EOF");
        endservent();_exit(0);
    }
    require(waitpid(child,&status,0)==child && !status,"fork EOF status");
    require(!getservent(),"parent retains EOF after append");
    endservent();
    char buffer[256];struct servent value,*result=(void *)1;
    for(size_t i=0;i<sizeof(buffer);++i) buffer[i]=0x5a;
    value.port=123;
    require(getservbyport_r(network(43210),"tcp",&value,buffer+1,1,&result)==34 && !result,"reentrant small buffer reports ERANGE and clears result");
    require(value.port==123,"ERANGE must not publish partial servent");
    for(size_t i=0;i<sizeof(buffer);++i) require(buffer[i]==0x5a,"ERANGE must not write caller buffer");
    require(!getservbyport_r(network(43210),"tcp",&value,buffer+1,sizeof(buffer)-1,&result) && result==&value && equal(value.name,"alpha"),"reentrant lookup with unaligned buffer");
    require((unsigned long)value.aliases%sizeof(void *)==0 && equal(value.aliases[0],"alias") && !value.aliases[1],"aligned reentrant alias vector");
    result=(void *)1;require(!getservbyport_r(network(9999),"tcp",&value,buffer,sizeof(buffer),&result) && !result,"reentrant missing entry");
    require(!getservbyname_r("alias","tcp",&value,buffer,sizeof(buffer),&result) && result==&value && value.port==network(43210),"reentrant service alias");
    require(!getservbyname_r("ALIAS","tcp",&value,buffer,sizeof(buffer),&result) && !result,"service names are case sensitive");
    require(!getaddrinfo("127.0.0.1","alias",&hints,&answers) && answers,"named service resolved from guest file");
    for(struct addrinfo *p=answers;p;p=p->next) require(p->socktype==1 && ((unsigned short *)p->address)[1]==network(43210),"guest service addrinfo port");
    freeaddrinfo(answers);
    fixture("");require(!getservbyname("http","tcp"),"empty services file is authoritative");
    require(!rename("/etc/services","/etc/services.saved"),"hide services file");
    *__errno_location()=0;require(!getservbyport(network(80),"tcp") && *__errno_location()==2,"missing services after lookup has no cached defaults");
    write(1,"SERVICES_DB_OK\n",15);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
