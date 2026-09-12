typedef unsigned long size_t;
extern int unshare(int), socket(int,int,int), ioctl(int,unsigned long,...), open(const char *,int,...), close(int);
extern long read(int,void *,size_t), write(int,const void *,size_t), send(int,const void *,size_t,int), recv(int,void *,size_t,int);
extern long sendto(int,const void *,size_t,int,const void *,unsigned);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s) {size_t n=0;while(s[n])++n;return n;}
static void require(int ok,const char *why) {if(!ok){write(2,why,length(why));write(2,"\n",1);_exit(93);}}
static int contains(const char *s,const char *p) {
    for(size_t i=0;s[i];++i){size_t j=0;while(p[j] && s[i+j]==p[j])++j;if(!p[j])return 1;}return 0;
}
static unsigned lines(const char *s) {unsigned n=0;while(*s)n+=*s++=='\n';return n;}
static void contents(const char *path,char *buffer,size_t capacity) {
    int fd=open(path,0);require(fd>=0,"open network proc file");
    long n=read(fd,buffer,capacity-1);require(n>=0 && (size_t)n<capacity-1 && !close(fd),"read network proc file");buffer[n]=0;
}
static void route(int kind) {
    struct attribute {unsigned short length,kind;unsigned value;};
    struct {
        unsigned length;unsigned short kind,flags;unsigned sequence,pid;
        unsigned char family,prefix,source,tos,table,protocol,scope,type;unsigned route_flags;
        struct attribute destination,interface,metric;
    } message={
        .length=52,.kind=(unsigned short)kind,.flags=kind==24 ? 0x605 : 5,.sequence=42,
        .family=2,.prefix=24,.table=254,.protocol=4,.scope=253,.type=1,
        .destination={8,1,0x0006070a},.interface={8,4,1},.metric={8,6,23}
    };
    _Static_assert(sizeof(message)==52,"netlink route frame");
    int fd=socket(16,3,0);require(fd>=0,"route netlink socket");
    struct {unsigned short family,padding;unsigned pid,groups;} kernel={.family=16};
    require(sendto(fd,&message,sizeof(message),0,&kernel,sizeof(kernel))==sizeof(message),"send route mutation");
    unsigned response[64];long n=recv(fd,response,sizeof(response),0);
    require(n>=20 && ((unsigned short *)response)[2]==2 && (int)response[4]==0,"route mutation ACK");
    require(!close(fd),"close route netlink");
}
__attribute__((used,noinline,noreturn)) static void probe_start(void) {
    char buffer[16384];require(!unshare(0x40000000),"private route namespace");
    contents("/proc/net/route",buffer,sizeof(buffer));require(lines(buffer)==1,"new namespace must not contain fixed or host routes");
    contents("/proc/net/arp",buffer,sizeof(buffer));require(lines(buffer)==1,"new namespace must not contain fake ARP entries");
    int control=socket(2,2,0);require(control>=0,"interface socket");
    struct {char name[16];short flags;char padding[22];} req={.name="lo",.flags=1};
    require(!ioctl(control,0x8914,&req),"enable loopback for route");
    route(24);contents("/proc/net/route",buffer,sizeof(buffer));
    require(contains(buffer,"lo\t0006070A\t00000000\t0001\t0\t0\t23\t00FFFFFF"),"netlink route appears in procfs");
    route(25);contents("/proc/net/route",buffer,sizeof(buffer));require(!contains(buffer,"0006070A"),"deleted route disappears from procfs");
    require(!close(control),"close interface socket");write(1,"PROCNET_ROUTE_OK\n",17);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {__asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");}
