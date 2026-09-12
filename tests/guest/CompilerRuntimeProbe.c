#define _GNU_SOURCE
#include <assert.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int (*increment)(void);
extern int __xpg_strerror_r(int,char *,size_t);
static void error_messages(void) {
    char buffer[64],small[4]={'x','x','x','z'};
    errno=1234;
    char *message=strerror_r(ENOENT,buffer,sizeof(buffer));
    assert(message && !strcmp(message,"No such file or directory"));
    assert(strerror_r(ENOENT,small,0)==message && small[0]=='x');
    assert(strerror_r(INT_MIN,buffer,sizeof(buffer))==buffer);
    assert(!strcmp(buffer,"Unknown error -2147483648"));
    assert(strerror_r(4096,small,3)==small && !strcmp(small,"Un") && small[3]=='z');
    assert(__xpg_strerror_r(ENOENT,buffer,sizeof(buffer))==0 && !strcmp(buffer,message));
    assert(__xpg_strerror_r(ENOENT,small,3)==ERANGE && !strcmp(small,"No") && small[3]=='z');
    assert(__xpg_strerror_r(ENOENT,NULL,0)==ERANGE);
    assert(__xpg_strerror_r(-1,buffer,sizeof(buffer))==EINVAL && !strcmp(buffer,"Unknown error -1"));
    assert(__xpg_strerror_r(4096,small,3)==EINVAL && !strcmp(small,"Un"));
    assert(errno==1234);
}
static void *thread(void *unused) {
    (void)unused;
    assert(increment()==41);
    return (void *)19;
}
int main(int argc,char **argv) {
    assert(argc==2);
    error_messages();
    char swapped[8];memset(swapped,'?',sizeof swapped);
    swab("abcde",swapped+1,5);assert(!memcmp(swapped,"?badc???",8));
    swab("xyz",swapped,-1);assert(!memcmp(swapped,"?badc???",8));
    FILE *file=tmpfile();assert(file);
    assert(fputs("compiler runtime\n",file)>=0 && !fflush(file));
    rewind(file);char buffer[64];assert(fgets(buffer,sizeof(buffer),file));
    assert(!strcmp(buffer,"compiler runtime\n") && !fclose(file));
    file=tmpfile();assert(file);
    assert(fputs("1 2 3 4 5 6 7 0x2a 1.25\n",file)>=0 && !fflush(file));
    rewind(file);int saved_stdin=dup(0);assert(saved_stdin>=0);
    assert(dup2(fileno(file),0)==0);
    int a,b,c,d,e,f,g;unsigned hex;double decimal;
    assert(scanf("%d %d %d %d %d %d %d %x %lf",&a,&b,&c,&d,&e,&f,&g,&hex,&decimal)==9);
    assert(a==1 && b==2 && c==3 && d==4 && e==5 && f==6 && g==7 && hex==42 && decimal==1.25);
    assert(dup2(saved_stdin,0)==0 && !close(saved_stdin) && !fclose(file));
    volatile long double number=2.5L-0x1p-62L;
    assert(llroundl(number)==2);
    Dl_info info;
    assert(dladdr((void *)llroundl,&info));
    assert(((unsigned char *)info.dli_fbase)[0]=='M' && ((unsigned char *)info.dli_fbase)[1]=='Z');
    void *library=dlopen(argv[1],RTLD_NOW);assert(library);
    increment=(int (*)(void))dlsym(library,"increment");assert(increment && increment()==41);
    int *ready=dlsym(library,"ready");assert(ready && *ready==9);
    pthread_t worker;void *result=0;
    assert(!pthread_create(&worker,0,thread,0) && !pthread_join(worker,&result) && result==(void *)19);
    pid_t child=fork();assert(child>=0);
    if(child==0) { assert(increment()==42 && *ready==9);_exit(23); }
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==23);
    assert(increment()==42 && !dlclose(library));
    puts("COMPILER_RUNTIME_OK");return 0;
}
