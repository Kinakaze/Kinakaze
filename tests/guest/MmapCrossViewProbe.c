#define _GNU_SOURCE
#include <sys/mman.h>
#include <stdio.h>
#include <string.h>
#include <assert.h>
#include <errno.h>
#include <sys/wait.h>
#include <unistd.h>
int main(void){
 size_t m=1024*1024,n=32*m;
 char *p=mmap(0,n,PROT_NONE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);assert(p!=MAP_FAILED);
 assert(mmap(p,n,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p);
 memset(p,0x5a,n);
 char *q=mmap(p+15*m,5*m,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0);
 if(q==MAP_FAILED){fprintf(stderr,"CROSS_VIEW_MMAP_FAILED errno=%d\n",errno);return 1;}
 for(size_t i=0;i<n;i++)assert((unsigned char)p[i]==(i>=15*m&&i<20*m?0:0x5a));
 memset(p,0x5a,n);
 int pid=fork();assert(pid>=0);
 if(!pid){
  assert(mmap(p+15*m,5*m,PROT_READ,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p+15*m);
  for(size_t i=0;i<n;i++)assert((unsigned char)p[i]==(i>=15*m&&i<20*m?0:0x5a));
  _exit(23);
 }
 int status;assert(waitpid(pid,&status,0)==pid && status==(23<<8));
 for(size_t i=0;i<n;i++)assert((unsigned char)p[i]==0x5a);
 assert(mmap(p+15*m,5*m,PROT_NONE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p+15*m);
 assert(mmap(p+14*m,7*m,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p+14*m);
 for(size_t i=0;i<n;i++)assert((unsigned char)p[i]==(i>=14*m&&i<21*m?0:0x5a));
 assert(!munmap(p,n));
 n=12*m;p=mmap(0,n,PROT_NONE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);assert(p!=MAP_FAILED);
 assert(mmap(p,4*m,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p);
 assert(mmap(p+8*m,4*m,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p+8*m);
 memset(p,0x3b,4*m);memset(p+8*m,0x3b,4*m);
 assert(mmap(p+m,10*m,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0)==p+m);
 for(size_t i=0;i<n;i++)assert((unsigned char)p[i]==(i>=m&&i<11*m?0:0x3b));
 assert(!munmap(p,n));puts("CROSS_VIEW_MMAP_OK");return 0;
}
