#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <dlfcn.h>
#include <time.h>
#include <fcntl.h>
#include <locale.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <unistd.h>
#include <wchar.h>

extern size_t __fread_unlocked_chk(void *, size_t, size_t, size_t, FILE *);
extern int __wctomb_chk(char *, wchar_t, size_t);
static void caught(int number) { (void)number; }
int main(void) {
    for (size_t alignment=1; alignment<=65536; alignment*=2) {
        unsigned char *p=aligned_alloc(alignment, 4097);
        assert(p && (uintptr_t)p%alignment==0);
        memset(p, 0xa5, 4097); assert(p[4096]==0xa5); free(p);
    }
    char *end;
    long double extended=strtold("18446744073709551615!",&end);
    assert(extended==18446744073709551615.0L && *end=='!');
    assert(strtold("0x1.0000000000000002p0",NULL)==0x1.0000000000000002p0L);
    errno=0; assert(strtold("1e4000",NULL)>1e3999L && errno==0);
    pthread_attr_t attr; int policy, inherit; struct sched_param param;
    assert(!pthread_attr_init(&attr));
    assert(!pthread_attr_setschedpolicy(&attr,SCHED_FIFO));
    param.sched_priority=17; assert(!pthread_attr_setschedparam(&attr,&param));
    assert(!pthread_attr_setinheritsched(&attr,PTHREAD_EXPLICIT_SCHED));
    assert(!pthread_attr_getschedpolicy(&attr,&policy) && policy==SCHED_FIFO);
    assert(!pthread_attr_getschedparam(&attr,&param) && param.sched_priority==17);
    assert(!pthread_attr_getinheritsched(&attr,&inherit) && inherit==PTHREAD_EXPLICIT_SCHED);
    pthread_attr_destroy(&attr);
    int (*create_timer)(clockid_t,struct sigevent *,timer_t *)=dlvsym(RTLD_DEFAULT,"timer_create","GLIBC_2.34");
    int (*set_timer)(timer_t,int,const struct itimerspec *,struct itimerspec *)=dlvsym(RTLD_DEFAULT,"timer_settime","GLIBC_2.34");
    int (*delete_timer)(timer_t)=dlvsym(RTLD_DEFAULT,"timer_delete","GLIBC_2.34");
    assert(create_timer && set_timer && delete_timer);
    struct sigevent event={.sigev_notify=SIGEV_NONE}; timer_t timer;
    assert(!create_timer(CLOCK_MONOTONIC,&event,&timer));
    struct itimerspec due={.it_value={10,0}}, old_due;
    assert(!set_timer(timer,0,&due,&old_due) && !old_due.it_value.tv_sec && !old_due.it_value.tv_nsec);
    assert(!delete_timer(timer)); errno=0; assert(delete_timer(timer)==-1 && errno==EINVAL);


    FILE *f=tmpfile(); assert(f); assert(fwrite("abcdef",1,6,f)==6); rewind(f);
    assert(fgetc(f)=='a');
    struct { fpos_t pos; uint64_t canary; } position={.canary=UINT64_C(0x1234567890abcdef)};
    assert(!fgetpos(f,&position.pos)); assert(fgetc(f)=='b');
    assert(!fsetpos(f,&position.pos)); assert(fgetc(f)=='b');
    char bytes[16]; assert(__fread_unlocked_chk(bytes,sizeof bytes,1,16,f)==4);
    assert(!memcmp(bytes,"cdef",4) && feof(f));
    assert((*(const int *)f & 0x10)!=0); /* actual GNU inline ABI */
    assert(position.canary==UINT64_C(0x1234567890abcdef));
    pid_t child=fork(); assert(child>=0);
    if (!child) _exit((*(const int *)f & 0x10) ? 0 : 2);
    int status; assert(waitpid(child,&status,0)==child && WIFEXITED(status) && !WEXITSTATUS(status));
    clearerr(f); assert(!(*(const int *)f & 0x30));
    assert(!fsetpos(f,&position.pos)); assert(fgetc(f)=='b'); fclose(f);
    f=tmpfile(); assert(f); assert(!setvbuf(f,NULL,_IONBF,0)); assert(!close(fileno(f))); assert(fputc('x',f)==EOF);
    assert(ferror(f) && (*(const int *)f & 0x20)); clearerr(f); assert(!(*(const int *)f & 0x30)); fclose(f);

    assert(setlocale(LC_CTYPE,"C.UTF-8"));
    assert(__wctomb_chk(bytes,0x20ac,sizeof bytes)==3 && !memcmp(bytes,"\xe2\x82\xac",3));
    f=tmpfile(); assert(f); assert(fputwc(L'\u00e9',f)==L'\u00e9');
    assert(fputwc(L'\u4e2d',f)==L'\u4e2d'); rewind(f); assert(fgetwc(f)==L'\u00e9');
    assert(!fgetpos(f,&position.pos)); assert(fgetwc(f)==L'\u4e2d');
    assert(!fsetpos(f,&position.pos)); assert(fgetwc(f)==L'\u4e2d'); fclose(f);

    int fd=open("stamp",O_CREAT|O_RDWR,0600); assert(fd>=0);
    struct timeval times[2]={{1700000000,123456},{1700000100,654321}};
    assert(!futimesat(AT_FDCWD,"stamp",times)); struct stat st; assert(!fstat(fd,&st));
    assert(st.st_mtim.tv_sec==times[1].tv_sec && st.st_mtim.tv_nsec==654321000);
    times[1].tv_sec++; assert(!futimesat(fd,NULL,times));
    assert(!fstat(fd,&st) && st.st_mtim.tv_sec==times[1].tv_sec);
    times[0].tv_usec=1000000; errno=0; assert(futimesat(fd,NULL,times)==-1 && errno==EINVAL);
    char *absolute=canonicalize_file_name("stamp"); assert(absolute && absolute[0]=='/'); free(absolute); close(fd);

    void (*old)(int)=sigset(SIGUSR1,caught); assert(old!=SIG_ERR);
    assert(sigset(SIGUSR1,SIG_HOLD)==caught); assert(sigset(SIGUSR1,SIG_HOLD)==SIG_HOLD);
    assert(sigset(SIGUSR1,caught)==SIG_HOLD); assert(sigset(SIGUSR1,old)==caught);
    puts("DESKTOP_ABI_POSITION_FLAGS_ALIGNMENT_SCHED_TIME_SIGNAL_OK");
}
