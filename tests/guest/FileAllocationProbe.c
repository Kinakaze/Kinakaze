#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void){
    char path[]="allocation-XXXXXX";int fd=mkstemp(path);assert(fd>=0);
    assert(write(fd,"preserved",9)==9);assert(lseek(fd,3,SEEK_SET)==3);
    struct stat before,after;assert(!fstat(fd,&before));
    assert(!fallocate(fd,1,0,2*1024*1024));assert(!fstat(fd,&after));
    assert(after.st_size==before.st_size && lseek(fd,0,SEEK_CUR)==3);
    assert(!fallocate64(fd,0,8192,4096));assert(!fstat(fd,&after) && after.st_size==12288);
    char data[64];assert(pread(fd,data,9,0)==9 && !memcmp(data,"preserved",9));
    memset(data,42,sizeof data);assert(pread(fd,data,sizeof data,8192)==sizeof data);
    for(unsigned i=0;i<sizeof data;i++)assert(!data[i]);
    assert(!fallocate(fd,0,0,1));assert(!fstat(fd,&after) && after.st_size==12288);
    errno=123;assert(!posix_fallocate(fd,0,1024) && errno==123);
    assert(posix_fallocate(fd,-1,4)==EINVAL && errno==123);
    errno=0;assert(fallocate(fd,0,-1,4)==-1 && errno==EINVAL);
    errno=0;assert(fallocate(fd,0,INT64_MAX,1)==-1 && errno==EFBIG);
    errno=0;assert(fallocate(fd,0x10,0,4)==-1 && errno==EOPNOTSUPP);
    int readonly=open(path,O_RDONLY);assert(readonly>=0);
    errno=0;assert(fallocate(readonly,0,0,4)==-1 && errno==EBADF);close(readonly);
    assert(!lchmod(path,0600));assert(!stat(path,&after) && (after.st_mode&0777)==0600);
    assert(!symlink(path,"allocation-link"));errno=0;
    assert(lchmod("allocation-link",0777)==-1 && errno==EOPNOTSUPP);
    assert(!stat(path,&after) && (after.st_mode&0777)==0600);
    assert(!unlink("allocation-link"));close(fd);assert(!unlink(path));

    char *secret=getpass("first: ");assert(secret && strlen(secret)==300);
    for(unsigned i=0;i<300;i++)assert(secret[i]=='p');
    pid_t child=fork();assert(child>=0);
    if(!child){char *next=getpass("child: ");assert(next==secret && !strcmp(next,"child"));_exit(0);}
    int status;assert(waitpid(child,&status,0)==child && status==0);
    assert(strlen(secret)==300 && secret[299]=='p');
    char *next=getpass("parent: ");assert(next==secret && !strcmp(next,"child"));
    puts("FILE_ALLOCATION_PASSWORD_FORK_OK");return 0;
}
