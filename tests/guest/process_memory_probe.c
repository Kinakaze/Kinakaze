/* Real Linux ELF: access a manager-authorized child, never a host PID. */
typedef unsigned long size_t;
typedef struct { void *base; size_t length; } iovec;
extern int fork(void), getpid(void), pipe(int[2]), close(int), waitpid(int,int*,int);
extern long read(int,void*,size_t), write(int,const void*,size_t);
extern long process_vm_readv(int,const iovec*,size_t,const iovec*,size_t,size_t);
extern long process_vm_writev(int,const iovec*,size_t,const iovec*,size_t,size_t);
extern long syscall(long,...);
extern int *__errno_location(void);
extern void _exit(int) __attribute__((noreturn));
static volatile int shared_address = 7;
static void require(int condition, const char *message) {
    if (!condition) { size_t n=0;while(message[n])n++;write(2,message,n);write(2,"\n",1);_exit(90); }
}
__attribute__((noreturn)) void probe_start(void) {
    int ready[2], finish[2];
    require(pipe(ready)==0 && pipe(finish)==0,"pipe");
    int child=fork();require(child>=0,"fork");
    if (!child) {
        close(ready[0]);close(finish[1]);shared_address=42;
        require(write(ready[1],"r",1)==1,"ready");char c;
        require(read(finish[0],&c,1)==1,"finish");
        require(shared_address==99,"remote write did not update child memory");_exit(0);
    }
    close(ready[1]);close(finish[0]);char c;
    require(read(ready[0],&c,1)==1,"wait ready");
    int value=0;
    iovec local={&value,sizeof(value)},remote={(void*)&shared_address,sizeof(value)};
    require(process_vm_readv(child,&local,1,&remote,1,0)==sizeof(value) && value==42,"remote read");
    value=99;require(process_vm_writev(child,&local,1,&remote,1,0)==sizeof(value),"remote write");
    require(shared_address==7,"remote write altered parent memory");
    require(write(finish[1],"f",1)==1,"release child");int status;
    require(waitpid(child,&status,0)==child && status==0,"child exit");
    require(process_vm_readv(child,&local,1,&remote,1,0)==-1 && *__errno_location()==3,"dead child ESRCH");
    value=0;require(syscall(310L,getpid(),&local,1UL,&remote,1UL,0UL)==sizeof(value) && value==7,"raw self read");
    *__errno_location()=123;
    require(process_vm_readv(getpid(),&local,1,&remote,1,0)==sizeof(value) && *__errno_location()==123,"success preserves errno");
    remote.base=(void*)1;
    require(process_vm_readv(getpid(),&local,1,&remote,1,0)==-1 && *__errno_location()==14,"invalid address EFAULT");
    write(1,"PROCESS_MEMORY_OK\n",18);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2");
}
