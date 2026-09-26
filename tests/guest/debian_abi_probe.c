/* Linux ELF ABI regression for the Debian daily-command dependency gap. */
typedef unsigned long size_t;
extern void _exit(int) __attribute__((noreturn));
extern long write(int,const void *,size_t);
extern int *__errno_location(void);
extern int strcmp(const char *,const char *);
extern int strncmp(const char *,const char *,size_t);
extern size_t strlen(const char *);
extern void *memset(void *,int,size_t);
extern void free(void *);
extern int snprintf(char *,size_t,const char *,...);
extern int swprintf(int *,size_t,const int *,...);
extern int setenv(const char *,const char *,int);
extern int unsetenv(const char *);
extern int getpid(void);
extern int getuid(void);
extern int fork(void);
extern int waitpid(int,int *,int);
extern int pipe(int *);
extern long read(int,void *,size_t);
extern int close(int);
extern void *dlopen(const char *,int);
extern void *dlvsym(void *,const char *,const char *);
extern long double nearbyintl(long double);
extern int fesetround(int), feclearexcept(int), fetestexcept(int), feraiseexcept(int);
extern void *fopen(const char *,const char *);
extern int fclose(void *);
extern size_t fwrite(const void *,size_t,size_t,void *);
extern char *tempnam(const char *,const char *);
extern int access(const char *,int);
extern int dn_comp(const char *,unsigned char *,int,unsigned char **,unsigned char **);
extern int dn_expand(const unsigned char *,const unsigned char *,const unsigned char *,char *,int);
struct sgrp { char *name,*password,**admins,**members; };
extern struct sgrp *fgetsgent(void *);
extern int getsgnam_r(const char *,struct sgrp *,char *,size_t,struct sgrp **);
struct ttyent { char *name,*getty,*type; int status; char *window,*comment; };
extern struct ttyent *getttynam(const char *);
extern int setttyent(void),endttyent(void);
extern struct ttyent *getttyent(void);
extern int utmpname(const char *);
extern void setutent(void),endutent(void);
extern int getutent_r(void *,void **);
struct ntptimeval { long sec,usec,maxerror,esterror,tai,reserved[4]; };
extern int ntp_gettimex(struct ntptimeval *);
struct scratch { void *data; size_t size; char space[1024]; } __attribute__((aligned(16)));
extern _Bool __libc_scratch_buffer_grow(struct scratch *);
struct wordexp { size_t count; char **words; size_t offset; };
extern int wordexp(const char *,struct wordexp *,int);
extern void wordfree(struct wordexp *);
struct sigset { unsigned long bits[16]; };
struct siginfo { int signal,error,code,pad,pid; unsigned uid; unsigned long value; char tail[96]; };
struct timespec { long sec,nsec; };
extern int sigemptyset(struct sigset *),sigaddset(struct sigset *,int);
extern int sigprocmask(int,const struct sigset *,struct sigset *);
extern int sigtimedwait(const struct sigset *,struct siginfo *,const struct timespec *);
extern int sigqueue(int,int,unsigned long);
extern void *signal(int,void *);
struct mq_attr { long flags,maxmsg,msgsize,curmsgs,pad[4]; };
extern int mq_open(const char *,int,...),mq_unlink(const char *),mq_close(int);
extern int mq_send(int,const char *,size_t,unsigned);
extern long mq_receive(int,char *,size_t,unsigned *);
extern unsigned __nss_hash(const void *,size_t);
extern void *__nss_files_fopen(const char *);
extern int __open64_nocancel(const char *,int,...),__close_nocancel(int);
extern long __read_nocancel(int,void *,size_t),__pread64_nocancel(int,void *,size_t,long);
extern void *__mmap(void *,size_t,int,int,int,long);
extern int __munmap(void *,size_t),__madvise(void *,size_t,int);
extern unsigned long __strtoull_internal(const char *,char **,int,int);
extern char *__libc_secure_getenv(const char *);
extern void __lll_lock_wait_private(int *),__lll_lock_wake_private(int *);
struct xdr { int operation; void **ops; char *public,*private,*base; unsigned handy; };
extern void xdrmem_create(struct xdr *,char *,unsigned,int),xdrstdio_create(struct xdr *,void *,int);
extern int xdr_int(struct xdr *,int *),xdr_u_int(struct xdr *,unsigned *),xdr_bool(struct xdr *,int *);
extern int xdr_string(struct xdr *,char **,unsigned);
extern int xdr_array(struct xdr *,void **,unsigned *,unsigned,unsigned,void *);
extern void xdr_free(void *,void *);
extern unsigned long xdr_sizeof(void *,void *);
static void require(int ok,const char *message);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_join(unsigned long,void **);
extern int usleep(unsigned);
struct private_lock { int lock, acquired; };
static void *lock_thread(void *argument) {
    struct private_lock *state=argument;
    __lll_lock_wait_private(&state->lock);
    __atomic_store_n(&state->acquired,1,__ATOMIC_RELEASE);
    __atomic_store_n(&state->lock,0,__ATOMIC_RELEASE);
    __lll_lock_wake_private(&state->lock); return 0;
}
static void legacy(void) {
    require(__nss_hash("abc",3)==807794786U && __nss_hash(0,0)==0,"NSS hash ABI");
    void *database=__nss_files_fopen("/etc/gshadow"); require(database!=0 && fclose(database)==0,"NSS file open");
    int fd=__open64_nocancel("/etc/gshadow",0); char data[16]; require(fd>=0,"nocancel open");
    require(__pread64_nocancel(fd,data,7,2)==7 && !strncmp(data,"comment",7),"nocancel positional read");
    require(__read_nocancel(fd,data,1)==1 && data[0]=='#' && __close_nocancel(fd)==0,"nocancel read position and close");
    char *end; require(__strtoull_internal("0xffffffffffffffff!",&end,0,0)==~0UL && *end=='!',"legacy unsigned conversion");
    setenv("ABI_LEGACY","yes",1); require(!strcmp(__libc_secure_getenv("ABI_LEGACY"),"yes"),"legacy secure getenv");
    unsigned char *mapping=__mmap(0,4096,3,0x22,-1,0); require(mapping!=(void *)-1 && mapping[0]==0,"private mmap");
    mapping[0]=42; require(__madvise(mapping,4096,4)==0 && mapping[0]==0 && __munmap(mapping,4096)==0,"private madvise munmap");
    int lock=0; __lll_lock_wait_private(&lock); require(lock==2,"private lock acquisition");
    __atomic_store_n(&lock,0,__ATOMIC_RELEASE); __lll_lock_wake_private(&lock);
    struct private_lock state={1,0}; unsigned long thread;
    require(pthread_create(&thread,0,lock_thread,&state)==0,"private lock waiter thread");
    for(int attempt=0;attempt<1000 && __atomic_load_n(&state.lock,__ATOMIC_ACQUIRE)!=2;attempt++) usleep(1000);
    require(__atomic_load_n(&state.lock,__ATOMIC_ACQUIRE)==2 && !__atomic_load_n(&state.acquired,__ATOMIC_ACQUIRE),"private lock waits while owned");
    __atomic_store_n(&state.lock,0,__ATOMIC_RELEASE); __lll_lock_wake_private(&state.lock);
    require(pthread_join(thread,0)==0 && state.acquired && state.lock==0,"private lock release wakes waiter");
    write(1,"legacy ok\n",10);
}
static void xdrs(void) {
    char wire[128]={0}; struct xdr stream; int value=-17,truth=7; char *word="rpc-data";
    xdrmem_create(&stream,wire,sizeof wire,0);
    require(xdr_int(&stream,&value) && xdr_bool(&stream,&truth) && xdr_string(&stream,&word,32),"XDR encode");
    require((unsigned char)wire[0]==255 && (unsigned char)wire[3]==239 && wire[7]==1 && wire[11]==8,"XDR network byte order");
    require(xdr_sizeof((void *)xdr_int,&value)==4,"XDR encoded size");
    ((void (*)(struct xdr *))stream.ops[7])(&stream);
    xdrmem_create(&stream,wire,sizeof wire,1); value=0;truth=0;word=0;
    require(xdr_int(&stream,&value) && value==-17 && xdr_bool(&stream,&truth) && truth==1 && xdr_string(&stream,&word,32) && !strcmp(word,"rpc-data"),"XDR decode allocation");
    free(word); ((void (*)(struct xdr *))stream.ops[7])(&stream);
    unsigned count=3; int values[3]={1,-2,3}; void *array=values;
    xdrmem_create(&stream,wire,sizeof wire,0); require(xdr_array(&stream,&array,&count,3,4,(void *)xdr_int),"XDR array encode");
    ((void (*)(struct xdr *))stream.ops[7])(&stream); array=0;count=0;
    xdrmem_create(&stream,wire,sizeof wire,1); require(xdr_array(&stream,&array,&count,3,4,(void *)xdr_int) && count==3 && ((int *)array)[1]==-2,"XDR array decode");
    free(array); ((void (*)(struct xdr *))stream.ops[7])(&stream);
    xdrmem_create(&stream,wire,2,0); require(!xdr_int(&stream,&value),"XDR short buffer");
    ((void (*)(struct xdr *))stream.ops[7])(&stream);
    void *file=fopen("/tmp/abi.xdr","w+b"); require(file!=0,"XDR stdio fixture");
    xdrstdio_create(&stream,file,0); require(xdr_int(&stream,&value),"XDR stdio write");
    ((void (*)(struct xdr *))stream.ops[7])(&stream); require(fclose(file)==0,"XDR stdio close");
    write(1,"xdr ok\n",7);
}

static void require(int ok,const char *message) {
    if(ok) return;
    write(2,message,strlen(message)); write(2,"\n",1); _exit(1);
}
static void file(const char *path,const void *bytes,size_t count) {
    void *f=fopen(path,"wb"); require(f!=0,"fixture open");
    require(fwrite(bytes,1,count,f)==count,"fixture write"); require(fclose(f)==0,"fixture close");
}
static void formatting(void) {
    char b[256];
    snprintf(b,sizeof b,"%.0Lf",2048.L); require(!strcmp(b,"2048"),"long double fixed");
    snprintf(b,sizeof b,"%.0Lf",18446744073709551615.L); require(!strcmp(b,"18446744073709551615"),"long double integer precision");
    snprintf(b,sizeof b,"%.1Lf %.1Lf %.0Lf",1.25L,1.75L,.75L); require(!strcmp(b,"1.2 1.8 1"),"long double ties even");
    snprintf(b,sizeof b,"%d%d%d%d%d%d%d %.0Lf %.1f %s",1,2,3,4,5,6,7,2048.L,2.5,"end");
    require(!strcmp(b,"1234567 2048 2.5 end"),"mixed stack SSE and long double varargs");
    snprintf(b,sizeof b,"%3$.*2$Lf:%1$d",7,2,1.25L); require(!strcmp(b,"1.25:7"),"positional long double");
    snprintf(b,sizeof b,"%.3Le",1e4000L); require(!strcmp(b,"1.000e+4000"),"long double exponent range");
    snprintf(b,sizeof b,"%La",1.5L); require(!strcmp(b,"0xcp-3"),"long double hexadecimal");
    int wide[32]; static const int fmt[]={ '%','.', '0','L','f',0 };
    require(swprintf(wide,32,fmt,2048.L)==4 && wide[0]=='2' && wide[3]=='8',"wide long double");
    require(fesetround(0x400)==0,"set downward rounding");
    feclearexcept(0x3f); require(nearbyintl(1.75L)==1.L,"nearbyintl downward");
    require(!(fetestexcept(0x3f)&0x20),"nearbyintl suppresses new inexact");
    feraiseexcept(0x20); require(nearbyintl(1.5L)==1.L && (fetestexcept(0x20)&0x20),"nearbyintl retains existing inexact");
    require(fesetround(0)==0,"restore rounding"); feclearexcept(0x3f);
    require(nearbyintl(2.5L)==2.L && nearbyintl(3.5L)==4.L,"nearbyintl ties even");
    write(1,"format math ok\n",15);
}
static void databases(void) {
    static const char shadow[]="# comment\nmalformed\nstaff:!:root,admin:alice,bob\n";
    file("/etc/gshadow",shadow,sizeof shadow-1);
    void *f=fopen("/etc/gshadow","r"); require(f!=0,"gshadow stream");
    struct sgrp *s=fgetsgent(f); require(s && !strcmp(s->name,"staff") && !strcmp(s->admins[1],"admin"),"fgetsgent record");
    require(fgetsgent(f)==0,"fgetsgent EOF"); fclose(f);
    char bytes[256]; struct sgrp out,*result=(void *)1;
    require(getsgnam_r("staff",&out,bytes+1,1,&result)==34 && !result,"gshadow small buffer ERANGE");
    require(getsgnam_r("staff",&out,bytes+1,255,&result)==0 && result==&out && !strcmp(out.members[1],"bob") && !out.members[2],"gshadow reentrant pointers");
    require(getsgnam_r("missing",&out,bytes,sizeof bytes,&result)==0 && !result,"gshadow absent record");
    static const char ttys[]="tty1 \"/sbin/getty -L\" vt100 off secure on window=\"/bin/x -n\" # console\n";
    file("/etc/ttys",ttys,sizeof ttys-1);
    struct ttyent *tty=getttynam("tty1");
    require(tty && !strcmp(tty->getty,"/sbin/getty -L") && tty->status==3 && !strcmp(tty->window,"/bin/x -n") && !strcmp(tty->comment,"console"),"tty quoted fields");
    require(setttyent()==1 && getttyent()!=0 && !getttyent() && endttyent()==1,"tty enumeration");
    unsigned char record[384]={0},copy[384]; record[0]=7; record[4]=42;
    file("/tmp/abi.utmp",record,sizeof record); require(utmpname("/tmp/abi.utmp")==0,"utmp name"); setutent();
    void *returned=0; require(getutent_r(copy,&returned)==0 && returned==copy && copy[0]==7 && copy[4]==42,"reentrant utmp");
    *__errno_location()=71; require(getutent_r(copy,&returned)==-1 && !returned && *__errno_location()==71,"utmp EOF preserves errno"); endutent();
    struct { struct ntptimeval time; unsigned long guard; } ntp; memset(&ntp,0,sizeof ntp); ntp.guard=0x123456789abcdef0UL;
    require(ntp_gettimex(&ntp.time)==5 && ntp.time.sec>1700000000 && ntp.time.usec>=0 && ntp.time.usec<1000000 && ntp.guard==0x123456789abcdef0UL,"ntp layout and unsynchronized state");
    struct scratch scratch; scratch.data=scratch.space; scratch.size=1024;
    require(__libc_scratch_buffer_grow(&scratch) && scratch.data!=scratch.space && scratch.size==2048,"scratch grow");
    scratch.size=~0UL; require(!__libc_scratch_buffer_grow(&scratch) && scratch.data==scratch.space && scratch.size==1024 && *__errno_location()==12,"scratch overflow resets ownership");
    setenv("TMPDIR","/tmp",1); char *name=tempnam("/missing-directory","abcdef");
    require(name && !strncmp(name,"/tmp/abcde",10) && access(name,0)==-1 && *__errno_location()==2,"tempnam prefix directory and absence"); free(name);
    write(1,"databases ok\n",13);
}

extern char *strcpy(char *,const char *);
struct passwd_record { char *name,*password; unsigned uid,gid; char *gecos,*home,*shell; };
struct group_record { char *name,*password; unsigned gid; char **members; };
struct shadow_record { char *name,*password; long changed,min,max,warn,inactive,expire; unsigned long flags; };
extern int _nss_files_parse_pwent(char *,struct passwd_record *,char *,size_t,int *);
extern int _nss_files_parse_grent(char *,struct group_record *,char *,size_t,int *);
extern int _nss_files_parse_spent(char *,struct shadow_record *,char *,size_t,int *);
extern struct shadow_record *getspnam(const char *),*getspent(void);
extern int getspnam_r(const char *,struct shadow_record *,char *,size_t,struct shadow_record **);
extern int getspent_r(struct shadow_record *,char *,size_t,struct shadow_record **);
extern void setspent(void),endspent(void);
struct netgrent { int kind; char *values[3]; void *data; size_t size,position; int first; void *known,*needed,*action; };
extern int __internal_setnetgrent(const char *,struct netgrent *);
extern int __internal_getnetgrent_r(char **,char **,char **,struct netgrent *,char *,size_t,int *);
extern void __internal_endnetgrent(struct netgrent *);
static void nss_private(void) {
    char buffer[256]="alice:x:1001:1002:Alice:/home/alice:/bin/bash"; int error=0;
    struct passwd_record pw; struct group_record gr; struct shadow_record sp,*found;
    require(_nss_files_parse_pwent(buffer,&pw,buffer,sizeof buffer,&error)==1 && pw.uid==1001 && pw.gid==1002 && !strcmp(pw.home,"/home/alice"),"NSS passwd parser");
    strcpy(buffer,"staff:x:1002:alice,bob");
    require(_nss_files_parse_grent(buffer,&gr,buffer,24,&error)==-1 && error==34,"NSS group ERANGE");
    strcpy(buffer,"staff:x:1002:alice,bob");
    require(_nss_files_parse_grent(buffer,&gr,buffer,sizeof buffer,&error)==1 && gr.gid==1002 && !strcmp(gr.members[1],"bob") && !gr.members[2],"NSS group parser");
    strcpy(buffer,"alice:!locked:123:2:456:8:9:987:0");
    require(_nss_files_parse_spent(buffer,&sp,buffer,sizeof buffer,&error)==1 && sp.changed==123 && sp.expire==987,"NSS shadow parser");
    const char data[]="# comment\nalice:!locked:123:2:456:8:9:987:0\nbob:*:::::::\n";
    file("/etc/shadow",data,sizeof data-1);
    found=getspnam("alice"); require(found && !strcmp(found->password,"!locked") && found->changed==123 && found->min==2 && found->max==456 && found->warn==8 && found->inactive==9 && found->expire==987 && found->flags==0,"shadow actual aging fields");
    require(getspnam_r("alice",&sp,buffer,1,&found)==34 && !found,"shadow lookup ERANGE");
    require(getspnam_r("alice",&sp,buffer,sizeof buffer,&found)==0 && found==&sp && sp.changed==123,"shadow reentrant lookup");
    require(getspnam_r("root",&sp,buffer,sizeof buffer,&found)==0 && !found && !getspnam("root"),"no fabricated shadow for passwd account");
    setspent(); require(getspent_r(&sp,buffer,1,&found)==34 && !found,"shadow cursor ERANGE");
    require(getspent_r(&sp,buffer,sizeof buffer,&found)==0 && !strcmp(sp.name,"alice"),"shadow cursor retry");
    found=getspent(); require(found && !strcmp(found->name,"bob") && found->changed==-1 && found->expire==-1 && found->flags==~0UL,"shadow empty fields");
    require(!getspent(),"shadow enumeration EOF"); endspent();
    const char groups[]="one (host,alice,domain) nested\nnested (,bob,)\ntwo (other,carol,realm)\n";
    file("/etc/netgroup",groups,sizeof groups-1); struct netgrent first={0},second={0}; char *host,*user,*domain;
    require(__internal_setnetgrent("one",&first) && __internal_setnetgrent("two",&second),"independent netgroup states");
    require(!__internal_getnetgrent_r(&host,&user,&domain,&first,buffer,1,&error) && error==34,"netgroup ERANGE");
    require(__internal_getnetgrent_r(&host,&user,&domain,&second,buffer,sizeof buffer,&error) && !strcmp(user,"carol"),"second netgroup state");
    require(__internal_getnetgrent_r(&host,&user,&domain,&first,buffer,sizeof buffer,&error) && !strcmp(user,"alice"),"first netgroup retry state");
    require(__internal_getnetgrent_r(&host,&user,&domain,&first,buffer,sizeof buffer,&error) && !host && !domain && !strcmp(user,"bob"),"netgroup nested wildcards");
    require(!__internal_getnetgrent_r(&host,&user,&domain,&first,buffer,sizeof buffer,&error),"netgroup EOF");
    __internal_endnetgrent(&first); __internal_endnetgrent(&second);
    write(1,"NSS private ok\n",15);
}
extern void clnt_pcreateerror(const char *);
struct rpc_client { void *auth; void **ops; void *private; };
struct rpc_timeval { long sec,usec; };
extern char *strcpy(char *,const char *);
extern char *getenv(const char *);
extern int atoi(const char *);
extern struct rpc_client *__libc_clntudp_bufcreate(void *,unsigned long,unsigned long,struct rpc_timeval,int *,unsigned,unsigned,int);
extern int fcntl(int,int,...);
static void rpc_network(void) {
    char text[16]={0}; int port_fd=__open64_nocancel("/tmp/abi-rpc-port",0); require(port_fd>=0 && __read_nocancel(port_fd,text,15)>0 && __close_nocancel(port_fd)==0,"RPC fixture port");
    unsigned port=(unsigned)atoi(text); struct { unsigned short family,port; unsigned char ip[4],padding[8]; } address={2,(unsigned short)((port<<8)|(port>>8)),{127,0,0,1},{0}};
    int socket=-1; struct rpc_timeval retry={1,0},timeout={3,0};
    struct rpc_client *client=__libc_clntudp_bufcreate(&address,0x31234567,1,retry,&socket,1024,1024,0x80000);
    if(!client) clnt_pcreateerror("ABI UDP create");
    if(!client || socket<0 || !(fcntl(socket,1)&1)) { char message[128]; int n=snprintf(message,sizeof message,"RPC client %p fd %d errno %d flags %d\n",client,socket,*__errno_location(),fcntl(socket,1)); write(2,message,n); }
    require(client && socket>=0 && (fcntl(socket,1)&1),"RPC UDP create and close-on-exec");
    int argument=-17,result=0;
    int status=((int (*)(void *,unsigned long,void *,void *,void *,void *,struct rpc_timeval))client->ops[0])(client,7,(void *)xdr_int,&argument,(void *)xdr_int,&result,timeout);
    if(status) { char message[80]; int n=snprintf(message,sizeof message,"RPC status %d errno %d\n",status,*__errno_location()); write(2,message,n); }
    require(status==0 && result==-16,"RPC UDP wire round trip");
    ((void (*)(void *))client->ops[4])(client);
    write(1,"RPC network ok\n",15);
}
static void dns(void) {
    unsigned char packet[256]={0},*table[16]={packet}; char text[256];
    require(dn_comp("Example.COM",packet+12,244,table,table+16)==13,"DNS encode");
    require(dn_comp("www.example.com",packet+25,5,table,table+16)==-1 && *__errno_location()==90,"DNS overflow");
    require(dn_comp("www.example.com",packet+25,231,table,table+16)==6 && packet[29]==0xc0 && packet[30]==12,"DNS suffix compression");
    require(dn_expand(packet,packet+31,packet+25,text,sizeof text)==6 && !strcmp(text,"www.Example.COM"),"DNS round trip");
    int size=dn_comp("a\\.b.\\000x.",packet,256,0,0); require(size==8,"DNS escapes");
    require(dn_expand(packet,packet+size,packet,text,sizeof text)==size && !strcmp(text,"a\\.b.\\000x"),"DNS escaped round trip");
    require(dn_comp("bad..name",packet,256,0,0)==-1,"DNS invalid label");
    write(1,"dns ok\n",7);
}
static void words(void) {
    struct wordexp words={0,0,2}; setenv("ABI_WORDS","a b",1);
    int expansion=wordexp("\"a b\" $ABI_WORDS '' $((2+3))",&words,1|4|16);
    if(expansion) { char message[80]; int n=snprintf(message,sizeof message,"wordexp returned %d errno %d\n",expansion,*__errno_location()); write(2,message,n); }
    require(expansion==0,"wordexp expansion");
    require(words.count==5 && !words.words[0] && !words.words[1] && !strcmp(words.words[2],"a b") && !strcmp(words.words[3],"a") && !strcmp(words.words[4],"b") && !strcmp(words.words[5],"") && !strcmp(words.words[6],"5") && !words.words[7],"wordexp vector layout");
    require(wordexp("tail",&words,1|2|4)==0 && words.count==6 && !strcmp(words.words[7],"tail"),"wordexp append");
    require(wordexp("$(printf forbidden)",&words,1|2|4)==4 && words.count==6,"wordexp NOCMD preserves append");
    wordfree(&words); require(!words.words && words.count==0,"wordfree clears result");
    require(wordexp("$(printf allowed)",&words,0)==0 && words.count==1 && !strcmp(words.words[0],"allowed"),"wordexp command substitution"); wordfree(&words);
    unsetenv("KINAKAZE_UNDEFINED_ABI_TEST"); require(wordexp("$KINAKAZE_UNDEFINED_ABI_TEST",&words,32)==3,"wordexp undefined variable");
    require(wordexp("literal;touch /tmp/wordexp-invalid",&words,0)==2 && access("/tmp/wordexp-invalid",0)==-1,"wordexp rejects statements");
    require(wordexp("$((1+$(touch /tmp/wordexp-invalid)))",&words,4)==4 && access("/tmp/wordexp-invalid",0)==-1,"wordexp nested NOCMD");
    require(wordexp("'unfinished",&words,0)==5,"wordexp unterminated quote");
    require(wordexp("",&words,0)==0 && words.count==0 && words.words && !words.words[0],"wordexp empty input"); wordfree(&words);
    write(1,"wordexp ok\n",11);
}
static void signals(void) {
    struct sigset set,old; sigemptyset(&set); sigaddset(&set,10); sigaddset(&set,35);
    require(sigprocmask(0,&set,&old)==0,"block queued signals");
    require(sigqueue(getpid(),10,111)==0 && sigqueue(getpid(),10,222)==0,"standard queued signals");
    struct siginfo info; struct timespec timeout={3,0};
    require(sigtimedwait(&set,&info,&timeout)==10 && info.code==-1 && info.pid==getpid() && info.uid==(unsigned)getuid() && info.value==111,"sigqueue standard retains first payload");
    struct timespec zero={0,0}; require(sigtimedwait(&set,&info,&zero)==-1 && *__errno_location()==11,"standard signal coalescing");
    int channel[2]; require(pipe(channel)==0,"signal fixture pipe"); int parent=getpid(),child=fork(); require(child>=0,"signal fork");
    if(child==0) { close(channel[0]); for(int n=0;n<8;n++) require(sigqueue(parent,35,0x1234567800000000UL+n)==0,"child sigqueue send"); write(channel[1],"x",1); close(channel[1]); _exit(0); }
    close(channel[1]); char byte; long received=read(channel[0],&byte,1);
    require(received==1,"blocked queued signals do not interrupt pipe read"); close(channel[0]);
    for(int n=0;n<8;n++) {
        require(sigtimedwait(&set,&info,&timeout)==35 && info.code==-1 && info.pid==child && info.value==0x1234567800000000UL+n,"cross process queued FIFO and 64-bit payload");
    }
    require(sigtimedwait(&set,&info,&zero)==-1 && *__errno_location()==11,"no duplicate real-time delivery");
    require(sigqueue(getpid(),35,1)==0,"pending signal before ignore");
    require(signal(35,(void *)1)!=(void *)-1,"ignore pending queued signal");
    require(signal(35,0)!=(void *)-1,"restore queued signal disposition");
    require(sigtimedwait(&set,&info,&zero)==-1 && *__errno_location()==11,"ignored pending queue is discarded");
    int status; require(waitpid(child,&status,0)==child && status==0,"signal sender exit");
    require(sigqueue(0x7fffffff,35,0)==-1 && *__errno_location()==3,"sigqueue absent process");
    require(sigprocmask(2,&old,0)==0,"restore signal mask");
    write(1,"signals ok\n",11);
}
static void queues(void) {
    void *lib=dlopen("libc.so.6",2); require(lib && dlvsym(lib,"mq_receive","GLIBC_2.34") && dlvsym(lib,"mq_open","GLIBC_2.34") && dlvsym(lib,"mq_unlink","GLIBC_2.34"),"mq modern libc versions");
    char name[64]; snprintf(name,sizeof name,"/abi-queue-%d",getpid()); struct mq_attr attr={0,4,64,0,{0}};
    int fd=mq_open(name,0x40|0x80|2|0x800,0600,&attr); require(fd>=0,"mq open");
    require(mq_send(fd,"payload",7,31)==0,"mq send"); char data[64]; unsigned priority=0;
    require(mq_receive(fd,data,sizeof data,&priority)==7 && !strncmp(data,"payload",7) && priority==31,"mq receive");
    require(mq_unlink(name)==0 && mq_close(fd)==0,"mq cleanup");
    write(1,"mqueue ok\n",10);
}

extern void *dlsym(void *,const char *);
extern __thread int errno;
static void *errno_thread(void *argument) {
    (void)argument; require(&errno==__errno_location(),"thread errno identity");
    errno=137; require(*__errno_location()==137,"thread errno direct write");
    require(close(-1)==-1 && errno==9,"thread errno syscall"); return 0;
}
static void native_tls(void) {
    require(&errno==__errno_location(),"main errno identity"); errno=73;
    unsigned long thread; require(pthread_create(&thread,0,errno_thread,0)==0 && pthread_join(thread,0)==0,"native TLS thread lifecycle");
    require(errno==73,"errno thread isolation");
    int child=fork(); require(child>=0,"errno fork");
    if(!child) { require(&errno==__errno_location() && errno==73,"fork errno identity and value"); errno=91; _exit(0); }
    int status; require(waitpid(child,&status,0)==child && status==0 && errno==73,"fork errno isolation");
    write(1,"native TLS ok\n",14);
}
struct nss_action { char *module; unsigned bits; };
extern int __nss_database_get(int,struct nss_action **);
extern void *__nss_lookup_function(struct nss_action *,const char *);
extern int ruserok_af(const char *,int,const char *,const char *,unsigned short);
extern int chmod(const char *,unsigned);
extern void *__curbrk,*sbrk(long);
extern int brk(void *);
extern void __tunable_get_val(int,void *,void (*)(const unsigned long *));
static unsigned long tuning_callback;
static void tuning(const unsigned long *value) { tuning_callback=*value; }
static void private_loader(void) {
    struct nss_action *actions;
    const char config[]="passwd_compat: files [NOTFOUND=return]\npasswd: files\n";
    file("/etc/nsswitch.conf",config,sizeof config-1);
    require(__nss_database_get(10,&actions) && actions && actions[0].module && !actions[1].module && ((actions[0].bits>>4)&3)==1,"NSS configured action list");
    void *address=__nss_lookup_function(actions,"getpwnam_r"); require(address!=0,"NSS function dispatch");
    struct passwd_record pw; char buffer[512]; int error;
    require(((int (*)(const char *,struct passwd_record *,char *,size_t,int *))address)("root",&pw,buffer,sizeof buffer,&error)==1 && pw.uid==0,"NSS files lookup status");
    require(((int (*)(const char *,struct passwd_record *,char *,size_t,int *))address)("root",&pw,buffer,1,&error)==-2 && error==34,"NSS files ERANGE status");
    const char trusted[]="127.0.0.1 remote\n"; file("/etc/hosts.equiv",trusted,sizeof trusted-1); require(chmod("/etc/hosts.equiv",0600)==0,"trust file mode");
    require(ruserok_af("127.0.0.1",0,"remote","root",2)==0,"trusted remote account");
    require(ruserok_af("127.0.0.1",0,"intruder","root",2)==-1,"remote account mismatch");
    require(chmod("/etc/hosts.equiv",0666)==0 && ruserok_af("127.0.0.1",0,"remote","root",2)==-1,"reject writable trust file");
    setenv("GLIBC_TUNABLES","glibc.malloc.perturb=123",1); unsigned long value=~0UL;
    __tunable_get_val(3,&value,tuning); require((unsigned)value==123 && (value>>32)==0xffffffffUL && tuning_callback==123,"private tunable width and callback");
    setenv("GLIBC_TUNABLES","glibc.cpu.hwcaps=-AVX:glibc.malloc.perturb=999",1);
    char *setting=0; __tunable_get_val(25,&setting,0); require(setting && !strcmp(setting,"-AVX"),"private tunable string lifetime");
    setenv("MALLOC_PERTURB_","017",1); value=0; __tunable_get_val(3,&value,0); require(value==15,"tunable bounds and legacy alias");
    unsetenv("GLIBC_TUNABLES"); unsetenv("MALLOC_PERTURB_");
    void *lib=dlopen("libmvec.so.1",2); require(lib!=0,"libmvec private loader data");
    typedef double pair __attribute__((vector_size(16)));
    pair (*vector_sin)(pair)=dlsym(lib,"_ZGVbN2v_sin"); require(vector_sin!=0,"libmvec IFUNC lookup");
    pair input={0,0.5},output=vector_sin(input); require(output[0]==0 && output[1]>0.479425538603 && output[1]<0.479425538605,"libmvec actual vector computation");
    char *base=sbrk(0); require(base!=(void *)-1 && __curbrk==base,"program break origin");
    require(sbrk(8192)==base && __curbrk==base+8192,"program break grow"); base[0]=23;base[4096]=45;
    int child=fork(); require(child>=0,"program break fork");
    if(!child) { require(sbrk(0)==base+8192 && base[0]==23 && base[4096]==45,"program break fork contents"); require(sbrk(4096)==base+8192,"child program break grow");_exit(0); }
    int status; require(waitpid(child,&status,0)==child && status==0 && sbrk(0)==base+8192,"program break fork isolation");
    require(brk(base+4096)==0 && brk(base+8192)==0 && base[4096]==0 && base[0]==23,"program break shrink discard");
    require(sbrk(-8192)==base+8192 && sbrk(0)==base && sbrk(-1)==(void *)-1 && *__errno_location()==12,"program break bounds");
    write(1,"private loader ok\n",18);
}
extern void *iconv_open(const char *,const char *);
extern size_t iconv(void *,char **,size_t *,char **,size_t *);
extern int iconv_close(void *);
extern int __gconv_transliterate(void *,void *,const char *,const char **,const char *,char **,size_t *);
static int gconv_step(void *step,void *data,const char **input,const char *end,char **output,size_t *irreversible,int flush,int incomplete) {
    (void)step;(void)irreversible; require(!flush && !incomplete,"gconv callback flags");
    if(*output==*(char **)data) return 5;
    require(end-*input==4 && *(const unsigned *)*input=='?',"gconv replacement passed through encoder");
    *(*output)++='?'; *input=end; return 4;
}
static void transliteration(void) {
    void *conversion=iconv_open("ASCII//TRANSLIT","UTF-8"); require(conversion!=(void *)-1,"C transliteration open");
    char source[]={ (char)0xc3,(char)0xa9,0 },destination[4];char *in=source,*out=destination;size_t in_left=2,out_left=0;
    require(iconv(conversion,&in,&in_left,&out,&out_left)==(size_t)-1 && *__errno_location()==7 && in_left==2,"transliteration E2BIG retry");
    out_left=sizeof destination;require(iconv(conversion,&in,&in_left,&out,&out_left)==1 && destination[0]=='?' && in_left==0,"C default replacement and irreversible count");
    require(iconv_close(conversion)==0,"transliteration close");
    unsigned long step[6]={0}; step[5]=(unsigned long)gconv_step;
    unsigned wide=0xe9; const char *begin=(const char *)&wide,*cursor=begin; char *output=destination,*limit=destination; size_t irreversible=0;
    require(__gconv_transliterate(step,&limit,begin,&cursor,begin+4,&output,&irreversible)==5 && cursor==begin && output==destination && irreversible==0,"private gconv short output is retryable");
    limit=destination+sizeof destination;
    require(__gconv_transliterate(step,&limit,begin,&cursor,begin+4,&output,&irreversible)==0 && cursor==begin+4 && output==destination+1 && irreversible==1,"private gconv callback commits conversion");
    require(__gconv_transliterate(step,&limit,begin,&cursor,begin+4,&output,&irreversible)==4,"private gconv empty input");
    cursor=begin; require(__gconv_transliterate(step,&limit,begin,&cursor,begin+3,&output,&irreversible)==7 && cursor==begin,"private gconv incomplete input");
    unsigned long guard; __asm__ volatile("movq %%fs:0x30,%0":"=r"(guard));
    step[0]=1; unsigned long encoded=(unsigned long)gconv_step^guard; step[5]=(encoded<<17)|(encoded>>(64-17));
    require(__gconv_transliterate(step,&limit,begin,&cursor,begin+4,&output,&irreversible)==0 && irreversible==2,"private gconv protected callback pointer");
    write(1,"transliteration ok\n",19);
}
__attribute__((noreturn)) void probe_start(void) {
    formatting(); databases(); dns(); words(); signals(); queues(); legacy(); xdrs(); nss_private(); rpc_network(); native_tls(); transliteration(); private_loader();
    write(1,"DEBIAN_ABI_OK\n",14); _exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("xor %rbp,%rbp\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
