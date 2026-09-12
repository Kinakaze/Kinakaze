typedef unsigned long size_t;
struct iovec { void *base; size_t len; };
extern int open(const char *,int,...),close(int),unlink(const char *),pipe(int *),dup(int);
extern int mkdir(const char *,unsigned),rmdir(const char *),mount(const char *,const char *,const char *,unsigned long,const void *),umount2(const char *,int);
extern long read(int,void *,size_t),write(int,const void *,size_t),lseek(int,long,int);
extern long pwrite64(int,const void *,size_t,long),pread64(int,void *,size_t,long);
extern long preadv64(int,const struct iovec *,int,long),pwritev64(int,const struct iovec *,int,long);
extern long preadv64v2(int,const struct iovec *,int,long,int),pwritev64v2(int,const struct iovec *,int,long,int);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));
static size_t length(const char *s){size_t n=0;while(s[n])++n;return n;}
static int equal(const char *a,const char *b,size_t n){for(size_t i=0;i<n;++i)if(a[i]!=b[i])return 0;return 1;}
static void require(int ok,const char *message){if(!ok){write(2,message,length(message));write(2,"\n",1);_exit(91);}}
static void file(const char *path) {
    int fd=open(path,2|64|512,0600);require(fd>=0,"create positioned file");
    require(write(fd,"abcdefgh",8)==8 && lseek(fd,6,0)==6,"initial file contents");
    int alias=dup(fd);require(alias>=0,"shared description alias");
    struct iovec out[]={{"12",2},{0,0},{"34",2}};
    require(pwritev64v2(fd,out,3,1,0)==4 && lseek(alias,0,1)==6,"pwritev2 preserves shared offset");
    char first[4]={0},last[4]={0};struct iovec in[]={{first,2},{last,3}};
    require(preadv64v2(fd,in,2,3,0)==5 && equal(first,"34",2) && equal(last,"fgh",3),"preadv2 scatters data");
    require(lseek(fd,0,1)==6,"preadv2 preserves offset");
    require(preadv64(fd,in,2,7)==1 && first[0]=='h' && last[0]=='f',"short preadv stops before next vector");
    require(pwrite64(fd,"Z",1,0)==1 && lseek(fd,0,1)==6,"pwrite preserves offset");
    struct iovec partial[]={{"Z",1},{0,1}};
    require(pwritev64v2(fd,partial,2,0,0)==1 && lseek(fd,0,1)==6,"partial write before later vector fault");
    partial[0].base=first;
    require(preadv64v2(fd,partial,2,0,0)==1 && first[0]=='Z',"partial read before later vector fault");
    require(pwritev64(fd,out,1,-1)==-1 && *__errno_location()==22,"legacy negative offset rejected");
    require(preadv64v2(fd,in,2,-2,0)==-1 && *__errno_location()==22,"v2 invalid offset");
    require(preadv64v2(fd,in,2,0,8)==-1 && *__errno_location()==95,"unsupported nowait reports EOPNOTSUPP");
    require(pwritev64v2(fd,out,1,0,2)==-1 && *__errno_location()==95,"unsupported sync flag does not write");
    struct iovec huge[]={{first,(size_t)-1},{last,1}};
    require(pwritev64v2(fd,huge,2,0,0)==-1 && *__errno_location()==22,"aggregate length overflow rejected");
    require(preadv64v2(-1,0,0,0,0)==-1 && *__errno_location()==9,"invalid fd with empty vector");
    require(preadv64v2(-1,0,0,-1,0)==-1 && *__errno_location()==9,"invalid stream fd with empty vector");
    require(preadv64v2(fd,0,0,0,0)==0,"zero vector count");
    require(pwritev64v2(fd,out,1,-1,0)==2 && lseek(alias,0,1)==8,"offset -1 advances shared position");
    require(lseek(fd,0,0)==0 && preadv64v2(fd,in,2,-1,0)==5 && lseek(alias,0,1)==5,"offset -1 read advances position");
    close(alias);close(fd);
    fd=open(path,1|1024);require(fd>=0,"append open");
    require(pwrite64(fd,"A",1,0)==1 && lseek(fd,0,1)==0,"Linux append pwrite does not update position");
    close(fd);fd=open(path,0);char data[10]={0};
    require(read(fd,data,10)==9 && equal(data,"Z1234f12A",9),"append and positional contents");
    require(pwrite64(fd,"x",1,0)==-1 && *__errno_location()==9,"read-only positional write fails");
    close(fd);require(unlink(path)==0,"remove fixture");
}
__attribute__((noreturn)) void probe_start(void){
    file("/positioned-native");
    require(mkdir("/ram",0700)==0,"tmpfs mount point");
    require(mount("tmpfs","/ram","tmpfs",0,"size=1m")==0,"mount actual tmpfs");
    file("/ram/positioned-tmpfs");
    require(umount2("/ram",0)==0 && rmdir("/ram")==0,"tmpfs cleanup");
    int fds[2];require(pipe(fds)==0,"pipe");
    struct iovec out[]={{"pipe",4}};char buffer[8]={0};struct iovec in[]={{buffer,8}};
    require(pwrite64(fds[1],"bad",3,0)==-1 && *__errno_location()==29,"pwrite pipe must not write");
    require(pwritev64v2(fds[1],out,1,0,0)==-1 && *__errno_location()==29,"pwritev2 pipe offset rejected");
    require(pwritev64v2(fds[1],out,1,-1,0)==4,"v2 pipe stream write");
    require(preadv64(fds[0],in,1,0)==-1 && *__errno_location()==29,"preadv pipe must not consume");
    close(fds[1]);require(preadv64v2(fds[0],in,1,-1,0)==4 && equal(buffer,"pipe",4),"v2 pipe stream read");close(fds[0]);
    int device=open("/dev/null",2);require(device>=0 && pwrite64(device,"discard",7,900)==7 && preadv64(device,in,1,99)==0,"null positional I/O");close(device);
    device=open("/dev/zero",0);require(device>=0 && preadv64v2(device,in,1,99,0)==8,"zero positional read");
    for(int i=0;i<8;++i)require(buffer[i]==0,"zero bytes");close(device);
    device=open("/dev/full",1);require(device>=0 && pwrite64(device,"x",1,0)==-1 && *__errno_location()==28,"full positional error");close(device);
    write(1,"POSITIONED_IO_OK\n",17);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void){__asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");}
