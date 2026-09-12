#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    size_t page=(size_t)sysconf(_SC_PAGESIZE);
    char *memory=mmap(NULL,page*4,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    assert(memory!=MAP_FAILED);
    memset(memory,41,page*4);
    assert(!mlock(memory+1,page));
    assert(!mlock(memory+page,page));
    assert(!munlock(memory+page+1,1));
    assert(!munlock(memory+page+1,1));
    assert(!syscall(SYS_mlock,memory+page*2,page));
    assert(!syscall(SYS_munlock,memory+page*2,page));
    assert(!mlock2(memory+page*2,page,0));
    errno=0;assert(mlock((void *)0,page)==-1 && errno==ENOMEM);
    errno=0;assert(mlockall(0)==-1 && errno==EINVAL);
    errno=0;assert(mlockall(MCL_ONFAULT)==-1 && errno==EINVAL);
    errno=0;assert(mlockall(MCL_FUTURE)==-1 && errno==EOPNOTSUPP);
    assert(!mprotect(memory+page*3,page,PROT_NONE));
    errno=0;assert(mlock(memory+page*3,page)==-1 && errno==ENOMEM);
    assert(!munlock(memory+page*3,page));
    assert(!mprotect(memory+page*3,page,PROT_READ|PROT_WRITE));
    pid_t child=fork();assert(child>=0);
    if(!child) {
        assert(!munlockall());
        assert(memory[0]==41 && memory[page*2]==41);
        assert(!mlock(memory,page));memory[0]=42;
        assert(!munlockall());assert(!munmap(memory,page*4));_exit(0);
    }
    int status;assert(waitpid(child,&status,0)==child && status==0);
    assert(memory[0]==41);
    assert(!munlockall());assert(!munmap(memory,page*4));
    puts("MEMORY_LOCK_RANGES_SYSCALLS_ERRORS_FORK_OK");
    return 0;
}
