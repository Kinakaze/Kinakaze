typedef unsigned long size_t;
extern int unshare(int), socket(int,int,int), ioctl(int,unsigned long,...);
extern int bind(int,const void *,unsigned), connect(int,const void *,unsigned);
extern int listen(int,int), accept(int,void *,unsigned *), getsockname(int,void *,unsigned *);
extern int setsockopt(int,int,int,const void *,unsigned), close(int);
extern long read(int,void *,size_t), write(int,const void *,size_t);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s) { size_t n=0;while(s[n]) ++n;return n; }
static void require(int ok,const char *why) {
    if(ok) return;
    write(2,why,length(why));
    int error=*__errno_location();char digits[12];unsigned n=0;
    do {digits[n++]=(char)('0'+error%10);error/=10;} while(error);
    write(2," errno=",7);while(n) write(2,&digits[--n],1);write(2,"\n",1);_exit(93);
}
static void marker(const char *text) { write(1,text,length(text)); }
struct address4 { unsigned short family,port; unsigned char ip[4],padding[8]; };
struct address6 { unsigned short family,port; unsigned flow; unsigned char ip[16]; unsigned scope; };
static void exchange(int server_family,int client_family) {
    int listener=socket(server_family,1,0);require(listener>=0,"create listener");
    if(server_family==10) {
        int only=0;require(!setsockopt(listener,41,26,&only,sizeof(only)),"enable dual-stack listener");
    }
    struct address4 local4={.family=2},peer4={.family=2,.ip={127,0,0,1}};
    struct address6 local6={.family=10},peer6={.family=10,.ip={[15]=1}};
    void *local=server_family==2 ? (void *)&local4 : (void *)&local6;
    unsigned size=server_family==2 ? sizeof(local4) : sizeof(local6);
    require(!bind(listener,local,size) && !listen(listener,4),"bind and listen");
    require(!getsockname(listener,local,&size),"listener port");
    peer4.port=peer6.port=server_family==2 ? local4.port : local6.port;
    int client=socket(client_family,1,0);require(client>=0,"create client");
    void *peer=client_family==2 ? (void *)&peer4 : (void *)&peer6;
    unsigned peer_size=client_family==2 ? sizeof(peer4) : sizeof(peer6);
    require(!connect(client,peer,peer_size),"connect loopback");
    struct address6 remote={0};unsigned remote_size=sizeof(remote);
    int accepted=accept(listener,&remote,&remote_size);require(accepted>=0,"accept loopback");
    if(server_family==10 && client_family==2) {
        require(remote.family==10 && remote.ip[10]==255 && remote.ip[11]==255
            && remote.ip[12]==127 && remote.ip[15]==1,"IPv4-mapped accepted peer");
    }
    char byte=0;require(write(client,"x",1)==1 && read(accepted,&byte,1)==1 && byte=='x',"loopback payload");
    require(!close(accepted) && !close(client) && !close(listener),"close loopback sockets");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    require(!unshare(0x40000000),"new network namespace");
    int control=socket(2,2,0);require(control>=0,"interface control socket");
    struct {char name[16];short flags;char padding[22];} request={.name="lo"};
    require(!ioctl(control,0x8913,&request),"read loopback flags");
    require(!(request.flags&1) && (request.flags&8),"new namespace loopback starts down");
    struct {int length;void *buffer;} conf={0};
    require(!ioctl(control,0x8912,&conf) && !conf.length,"unconfigured namespace has no IPv4 addresses");
    request.flags|=1;require(!ioctl(control,0x8914,&request),"enable loopback");
    request.flags=0;require(!ioctl(control,0x8913,&request) && (request.flags&1),"loopback up is real state");
    require(!ioctl(control,0x8912,&conf) && conf.length==40,"configured IPv4 address count");
    struct {char name[16];unsigned char data[24];} address={0};
    conf.buffer=&address;conf.length=sizeof(address);
    require(!ioctl(control,0x8912,&conf) && conf.length==40 && address.name[0]=='l'
        && address.data[0]==2 && address.data[4]==127 && address.data[7]==1,"ifconf returns namespace loopback address");
    request.name[0]='x';
    require(ioctl(control,0x8913,&request)==-1 && *__errno_location()==19,"unknown interface must return ENODEV");
    request.name[0]='l';
    exchange(2,2);marker("INET_NAMESPACE_IPV4_OK\n");
    exchange(10,10);marker("INET_NAMESPACE_IPV6_OK\n");
#ifdef PROBE_DUALSTACK
    exchange(10,2);marker("INET_NAMESPACE_DUALSTACK_OK\n");
#else
    require(!unshare(0x40000000),"second network namespace");
    int second=socket(2,2,0);require(second>=0,"second interface socket");
    require(!ioctl(second,0x8913,&request) && !(request.flags&1),"new namespace flags are independent");
    require(!ioctl(control,0x8913,&request) && (request.flags&1),"ioctl follows socket namespace after unshare");
    request.flags&=~1;require(!ioctl(control,0x8914,&request),"bring old namespace loopback down");
    request.flags=1;require(!ioctl(control,0x8913,&request) && !(request.flags&1),"interface down is real state");
    require(!close(control) && !close(second),"close namespace sockets");
    marker("INET_NAMESPACE_OK\n");
#endif
    _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
