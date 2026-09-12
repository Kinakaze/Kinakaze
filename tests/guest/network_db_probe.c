/* Real Linux ABI, guest database authority, TLS lifetime and fork pointers. */
typedef unsigned long size_t;
struct protoent { char *name; char **aliases; int protocol; };
struct netent { char *name; char **aliases; int family; unsigned network; };
extern struct protoent *getprotobyname(const char *), *getprotobynumber(int);
extern struct netent *getnetbyname(const char *), *getnetbyaddr(unsigned, int);
extern int getprotobyname_r(const char *,struct protoent *,char *,size_t,struct protoent **);
extern void setnetent(int),endnetent(void);
extern struct netent *getnetent(void);
extern unsigned inet_network(const char *);
extern unsigned char *ether_aton(const char *), *ether_aton_r(const char *, unsigned char *);
extern char *ether_ntoa(const unsigned char *), *ether_ntoa_r(const unsigned char *, char *);
extern int *__errno_location(void), *__h_errno_location(void);
extern int open(const char *, int, ...), close(int), rename(const char *, const char *);
extern long write(int, const void *, size_t);
extern int fork(void), waitpid(int, int *, int);
extern int pthread_create(unsigned long *, void *, void *(*)(void *), void *), pthread_join(unsigned long, void **);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s) { size_t n=0; while(s[n]) ++n; return n; }
static int equal(const char *a,const char *b) { while(*a && *a==*b) { ++a; ++b; } return *a==*b; }
static void require(int value,const char *why) { if(!value) { write(2,why,length(why));write(2,"\n",1);_exit(92); } }
static void file(const char *path,const char *text) {
    int fd=open(path,1|64|512,0600); require(fd>=0,"open fixture");
    require(write(fd,text,length(text))==(long)length(text),"write fixture");require(!close(fd),"close fixture");
}
static void *thread_probe(void *ignored) {
    (void)ignored;
    struct protoent *p=getprotobynumber(132);
    struct netent *n=getnetbyname("private");
    require(p && p->protocol==132 && equal(p->name,"sctp"),"thread protocol");
    require(n && n->network==0xc0a80000u,"thread network");
    unsigned char *mac=ether_aton("a:b:c:d:e:f");
    require(mac && equal(ether_ntoa(mac),"a:b:c:d:e:f"),"thread MAC");
    return (void *)1;
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    _Static_assert(sizeof(struct protoent)==24,"protoent layout");
    _Static_assert(sizeof(struct netent)==24,"netent layout");
    _Static_assert(__builtin_offsetof(struct netent,network)==20,"n_net offset");
    struct protoent *p=getprotobyname("TCP");
    require(p && p->protocol==6 && equal(p->name,"tcp"),"protocol alias");
    require(p->aliases && equal(p->aliases[0],"TCP") && !p->aliases[1],"protocol alias terminator");
    require(!getprotobyname("TcP") && !getprotobyname("udp") && !getprotobyname(""),"no default or case-folded protocol");
    require(!getprotobynumber(-1) && !getprotobynumber(999),"unknown protocol number");
    require(getprotobynumber(132)->protocol==132,"non TCP/UDP protocol");
    p=getprotobynumber(6);
    struct netent *n=getnetbyname("ALIAS");
    require(n && n->family==2 && n->network==0x0a000000u && equal(n->name,"TestNet"),"network name and host-order value");
    require(n->aliases && equal(n->aliases[0],"alias") && !n->aliases[1],"network aliases");
    require(getnetbyaddr(0xc0a80000u,0)->network==0xc0a80000u,"AF_UNSPEC and short network");
    require(!getnetbyaddr(0x0a000000u,10) && *__h_errno_location()==1,"unsupported network family");
    require(!getnetbyname("") && *__h_errno_location()==1,"empty network has no alias");
    n=getnetbyaddr(0x0a000000u,2);
    require(n && equal(n->name,"TestNet"),"network by address");
    require(inet_network("192.168")==0xc0a8u && inet_network("0300.0xA8.1.0")==0xc0a80100u,"inet_network byte order and bases");
    require(inet_network("1.256")==~0u && inet_network("1.2.3.4.5")==~0u,"inet_network rejects oversized octets");
    char buffer[256]; struct protoent reentrant,*answer=(void *)1;
    for(size_t i=0;i<sizeof(buffer);i++)buffer[i]=0x5a;
    reentrant.protocol=123;
    require(getprotobyname_r("TCP",&reentrant,buffer+1,1,&answer)==34 && !answer,"protocol short buffer ERANGE");
    require(reentrant.protocol==123,"protocol ERANGE preserves output");
    for(size_t i=0;i<sizeof(buffer);i++)require(buffer[i]==0x5a,"protocol ERANGE preserves bytes");
    require(!getprotobyname_r("TCP",&reentrant,buffer+1,sizeof(buffer)-1,&answer) && answer==&reentrant,"protocol reentrant alias");
    require(reentrant.protocol==6 && equal(reentrant.name,"tcp") && equal(reentrant.aliases[0],"TCP") && !reentrant.aliases[1],"protocol packed result");
    require((unsigned long)reentrant.aliases%sizeof(void *)==0,"protocol alias alignment");
    require(!getprotobyname_r("TcP",&reentrant,buffer,sizeof(buffer),&answer) && !answer,"protocol missing result");
    setnetent(1);struct netent *first=getnetent();require(first && equal(first->name,"TestNet"),"network enumeration first");
    int enumeration_child=fork();require(enumeration_child>=0,"fork network cursor");
    if(!enumeration_child) {
        require(equal(first->name,"TestNet"),"inherited enumeration record");
        struct netent *next=getnetent();require(next && equal(next->name,"private"),"child network cursor and unread buffer");
        require(!getnetent(),"child network EOF");endnetent();_exit(0);
    }
    int enumeration_status;require(waitpid(enumeration_child,&enumeration_status,0)==enumeration_child && !enumeration_status,"network enumeration fork status");
    require(getnetent()->network==0xc0a80000u && !getnetent(),"parent network cursor is independent");
    setnetent(0);require(equal(getnetent()->name,"TestNet"),"network rewind");endnetent();
    require(equal(getnetent()->name,"TestNet"),"network close and reopen");endnetent();
    n=getnetbyaddr(0x0a000000u,2);
    unsigned char guarded[8]={0xa5,0,0,0,0,0,0,0x5a};
    require(ether_aton_r("00:1:02:A:10:fF",guarded+1)==guarded+1,"MAC parse");
    require(guarded[0]==0xa5 && guarded[7]==0x5a && guarded[6]==255,"MAC parser bound");
    char text[20]; text[0]='!';text[19]='?';
    require(ether_ntoa_r(guarded+1,text+1)==text+1 && equal(text+1,"0:1:2:a:10:ff"),"MAC formatting");
    require(text[0]=='!' && text[19]=='?',"MAC formatter bound");
    require(!ether_aton_r("1:2:3:4:5:gg",guarded+1),"MAC bad hex");
    unsigned char *mac=ether_aton("0:1:2:a:10:ff");
    char *formatted=ether_ntoa(mac);
    require(mac && equal(formatted,"0:1:2:a:10:ff"),"static MAC storage");
    unsigned long thread; void *joined=0;
    require(!pthread_create(&thread,0,thread_probe,0) && !pthread_join(thread,&joined) && joined==(void *)1,"thread lookup");
    require(p->protocol==6 && equal(p->name,"tcp") && n->network==0x0a000000u,"thread database storage isolation");
    require(mac[0]==0 && equal(formatted,"0:1:2:a:10:ff"),"thread MAC storage isolation");
    int child=fork();require(child>=0,"fork");
    if(!child) {
        require(equal(p->name,"tcp") && equal(n->name,"TestNet"),"inherited database pointers");
        require(mac[5]==255 && equal(formatted,"0:1:2:a:10:ff"),"inherited MAC pointers");
        thread_probe(0);_exit(0);
    }
    int status=-1;require(waitpid(child,&status,0)==child && status==0,"fork lookup status");
    int baseline=open("/dev/null",0);require(baseline>=0 && !close(baseline),"fd baseline");
    for(int i=0;i<200;++i) require(getprotobynumber(6)!=0 && getnetbyname("alias")!=0,"repeated lookup");
    int after=open("/dev/null",0);require(after==baseline && !close(after),"lookup releases file descriptions");
    file("/etc/protocols","");
    require(!getprotobyname("tcp"),"empty protocol file is authoritative");
    require(!rename("/etc/protocols","/etc/protocols.saved"),"hide protocol file");
    require(!getprotobyname("tcp") && *__errno_location()==2,"missing protocol file has no fallback");
    file("/etc/protocols","newproto 99 custom\n");
    require(getprotobyname("custom")->protocol==99,"live protocol file replacement");
    file("/etc/networks","");*__errno_location()=123;
    require(!getnetbyname("loopback") && *__h_errno_location()==1 && *__errno_location()==123,"empty network file is authoritative");
    require(!rename("/etc/networks","/etc/networks.saved"),"hide network file");
    require(!getnetbyname("loopback") && *__errno_location()==2 && *__h_errno_location()==3,"missing network file error");
    file("/etc/networks","newnet 172.16 newalias\n");
    require(getnetbyname("NEWALIAS")->network==0xac100000u,"live network file replacement");
    write(1,"NETWORK_DB_OK\n",14);_exit(0);
}

__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
