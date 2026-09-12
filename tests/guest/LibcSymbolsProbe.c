#define _GNU_SOURCE
#include <assert.h>
#include <dirent.h>
#include <dlfcn.h>
#include <err.h>
#include <errno.h>
#include <fcntl.h>
#include <locale.h>
#include <math.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/timeb.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static pthread_barrier_t barrier;
static void *worker(void *unused) {
    (void)unused;int result=pthread_barrier_wait(&barrier);
    assert(result==0 || result==PTHREAD_BARRIER_SERIAL_THREAD);return (void *)29;
}
static void threads(void) {
    pthread_attr_t attr;assert(!pthread_attr_init(&attr));
    assert(!pthread_attr_setschedpolicy(&attr,SCHED_OTHER));
    struct sched_param p={0};assert(!pthread_attr_setschedparam(&attr,&p));
    assert(!pthread_attr_setinheritsched(&attr,PTHREAD_EXPLICIT_SCHED));
    assert(pthread_attr_setschedpolicy(&attr,99)==EINVAL);
    assert(!pthread_barrier_init(&barrier,NULL,2));
    pthread_t t;assert(!pthread_create(&t,&attr,worker,NULL));
    clockid_t clock;assert(!pthread_getcpuclockid(t,&clock));
    struct timespec value;assert(!clock_gettime(clock,&value));
    assert(value.tv_sec>=0 && value.tv_sec<1000);
    assert(!clock_getres(clock,&value));
    int r=pthread_barrier_wait(&barrier);assert(r==0 || r==PTHREAD_BARRIER_SERIAL_THREAD);
    void *result;assert(!pthread_join(t,&result) && result==(void *)29);
    assert(!pthread_barrier_destroy(&barrier));
    assert(!pthread_attr_setschedpolicy(&attr,SCHED_FIFO));p.sched_priority=1;
    assert(!pthread_attr_setschedparam(&attr,&p));
    assert(pthread_create(&t,&attr,worker,NULL)==EPERM);
    assert(!pthread_attr_destroy(&attr));
}
static void memory_streams(void) {
    char *a=NULL,*b=NULL;size_t na=0,nb=0;
    FILE *fa=open_memstream(&a,&na),*fb=open_memstream(&b,&nb);assert(fa && fb);
    assert(fputs("alpha",fa)>=0 && fputs("beta",fb)>=0);
    assert(!fflush(fa) && !fflush(fb));assert(na==5 && nb==4 && !strcmp(a,"alpha") && !strcmp(b,"beta"));
    pid_t child=fork();assert(child>=0);
    if(!child) { assert(fputs(" child",fa)>=0 && !fclose(fa));assert(!strcmp(a,"alpha child") && na==11);free(a);assert(!fclose(fb));free(b);_exit(0); }
    int status;assert(waitpid(child,&status,0)==child && !status);
    assert(fputs(" parent",fa)>=0 && !fclose(fa) && !fclose(fb));
    assert(na==12 && !strcmp(a,"alpha parent") && !strcmp(b,"beta"));free(a);free(b);
}
static void conversions(void) {
    char *end;errno=71;assert(strtod("0x1.8p2 ",&end)==6.0 && *end==' ' && errno==71);
    errno=0;assert(isinf(strtod("1e9999",&end)) && errno==ERANGE && !*end);
    locale_t locale=newlocale(LC_ALL_MASK,"C",NULL);assert(locale);
    assert(strtoll_l("-9223372036854775808!",&end,10,locale)==(-9223372036854775807LL-1) && *end=='!');
    assert(strtoull_l("18446744073709551615!",&end,10,locale)==~0ULL && *end=='!');freelocale(locale);
    char destination[12];memset(destination,0x55,sizeof destination);
    assert(memccpy(destination,"abc.def",'.',7)==destination+4 && !memcmp(destination,"abc.",4) && destination[4]==0x55);
    struct timeb now;assert(!ftime(&now) && now.time>1000000000 && now.millitm<1000);
    void *math=dlopen("libm.so.6",RTLD_NOW);assert(math);
    int (*signal_double)(double)=dlsym(math,"__issignaling");
    int (*signal_quad)(__float128)=dlsym(math,"__issignalingf128");
    int (*signal_extended)(long double)=dlsym(math,"__issignalingl");assert(signal_double && signal_quad && signal_extended);
    union { double value;unsigned long long bits; } d={.bits=0x7ff0000000000001ULL};
    union { __float128 value;unsigned long long bits[2]; } q={.bits={1,0x7fff000000000000ULL}};
    union { long double value;unsigned long long bits[2]; } l={.bits={0x8000000000000001ULL,0x7fff}};
    assert(signal_double(d.value)==1 && signal_double(INFINITY)==0);
    assert(signal_quad(q.value)==1 && signal_extended(l.value)==1);assert(!dlclose(math));
}
static void directory(void) {
    int fd=open(".",O_RDONLY|O_DIRECTORY);assert(fd>=0);
    struct dirent **entries=NULL;int count=scandirat(fd,".",&entries,NULL,versionsort);assert(count>=0);
    for(int i=0;i<count;i++)free(entries[i]);free(entries);close(fd);
}
int main(void) {
    conversions();threads();memory_streams();directory();
    pid_t child=fork();assert(child>=0);if(!child)errx(23,"symbol probe %d",17);
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==23);
    puts("LIBC_SYMBOLS_CLOCK_MEMORY_STREAM_FORK_OK");return 0;
}
