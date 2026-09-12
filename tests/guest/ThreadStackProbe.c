#define _GNU_SOURCE
#include <assert.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

/* Deliberately unprobed Linux frames, including Git's sub rsp,0x10048.
 * The red-zone store also requires the page immediately below aligned RSP. */
extern unsigned large_frame(size_t size, unsigned seed);
__asm__(
    ".text\n.type large_frame,@function\nlarge_frame:\n"
    "sub %rdi,%rsp\n"
    "mov %esi,(%rsp)\n"
    "mov %esi,-128(%rsp)\n"
    "mov $4096,%rcx\n"
    "1: cmp %rdi,%rcx\n"
    "jae 2f\n"
    "mov %esi,(%rsp,%rcx)\n"
    "add $4096,%rcx\n"
    "jmp 1b\n"
    "2: mov -128(%rsp),%eax\n"
    "xor (%rsp),%eax\n"
    "xor %esi,%eax\n"
    "add %rdi,%rsp\n"
    "ret\n.size large_frame,.-large_frame\n"
);

static void *worker(void *argument) {
    unsigned seed=(uintptr_t)argument;
    for(unsigned i=0;i<16;i++) {
        assert(large_frame(0x10048,seed+i)==seed+i);
        assert(large_frame(0x20048,seed+i)==seed+i);
        assert(large_frame(0x40048,seed+i)==seed+i);
    }
    return argument;
}

__attribute__((noinline)) static void *fork_frame(void *unused) {
    (void)unused;
    volatile unsigned char bytes[192*1024];
    for(size_t i=0;i<sizeof(bytes);i+=4096) bytes[i]=(unsigned char)(i/4096);
    pid_t child=fork();
    assert(child>=0);
    if(!child) {
        for(size_t i=0;i<sizeof(bytes);i+=4096) assert(bytes[i]==i/4096);
        assert(large_frame(0x10048,93)==93);
        pthread_t thread;
        assert(!pthread_create(&thread,NULL,worker,(void *)71));
        void *result;
        assert(!pthread_join(thread,&result) && result==(void *)71);
        _exit(0);
    }
    int status;
    assert(waitpid(child,&status,0)==child && status==0);
    for(size_t i=0;i<sizeof(bytes);i+=4096) assert(bytes[i]==i/4096);
    assert(large_frame(0x40048,94)==94);
    return (void *)95;
}

int main(void) {
    for(int explicit_size=0;explicit_size<2;explicit_size++) {
        pthread_attr_t attr;
        assert(!pthread_attr_init(&attr));
        if(explicit_size) assert(!pthread_attr_setstacksize(&attr,2*1024*1024));
        pthread_t threads[8];
        for(uintptr_t i=0;i<8;i++)
            assert(!pthread_create(&threads[i],&attr,worker,(void *)(i+1)));
        for(uintptr_t i=0;i<8;i++) {
            void *result;
            assert(!pthread_join(threads[i],&result) && result==(void *)(i+1));
        }
        assert(!pthread_attr_destroy(&attr));
    }
    pthread_t thread;
    void *result;
    assert(!pthread_create(&thread,NULL,fork_frame,NULL));
    assert(!pthread_join(thread,&result) && result==(void *)95);
    puts("THREAD_STACK_LARGE_FRAMES_RED_ZONE_FORK_OK");
    return 0;
}
