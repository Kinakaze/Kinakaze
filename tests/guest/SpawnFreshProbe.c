#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

static int hooks;
static void hook(void) { hooks++; }
static void caught(int signum) { (void)signum; }
static void wait_exit(pid_t pid,int code) {
    int status=-1; assert(waitpid(pid,&status,0)==pid);
    assert(WIFEXITED(status) && WEXITSTATUS(status)==code);
}
static int child(int argc,char **argv) {
    assert(argc==4 && getppid()==atoi(argv[2]));
    assert(getenv("FRESH_PROBE") && !strcmp(getenv("FRESH_PROBE"),"exact"));
    assert(!getenv("SHOULD_NOT_INHERIT"));
    assert(!getenv("KINAKAZE_V2_ADOPTION"));
    char cwd[1024]; assert(getcwd(cwd,sizeof(cwd)) && !strcmp(cwd,argv[3]));
    assert(umask(027)==027);
    char packet[4];assert(read(44,packet,4)==4 && !memcmp(packet,"ping",4));
    assert(write(44,"pong",4)==4);
    char b=0; assert(read(42,&b,1)==1 && b=='x');
    assert(fcntl(43,F_GETFD)==-1 && errno==EBADF);
    sigset_t mask,pending;struct sigaction action;
    assert(!sigprocmask(SIG_SETMASK,NULL,&mask) && !sigpending(&pending));
    assert(!sigismember(&pending,SIGUSR1));
    assert(!sigaction(SIGUSR1,NULL,&action) && action.sa_handler==SIG_DFL);
    assert(!sigaction(SIGUSR2,NULL,&action));
    if(!strcmp(argv[1],"explicit")) {
        assert(!sigismember(&mask,SIGUSR1) && sigismember(&mask,SIGUSR2));
        assert(action.sa_handler==SIG_DFL);
    } else {
        assert(sigismember(&mask,SIGUSR1) && !sigismember(&mask,SIGUSR2));
        assert(action.sa_handler==SIG_IGN);
    }
    // A fresh child's subsequent exec must not replay its adoption ticket.
    execl("/bin/sh","sh","-c","exit 23",(char *)NULL);
    return 99;
}
int main(int argc,char **argv) {
    if(argc>1) return child(argc,argv);
    char directory[]="/tmp/fresh-spawn-XXXXXX";assert(mkdtemp(directory));
    char path[1024];snprintf(path,sizeof(path),"%s/file",directory);
    int file=open(path,O_RDWR|O_CREAT|O_TRUNC,0600);assert(file>=0);
    assert(write(file,"xxxx",4)==4 && dup2(file,42)==42);
    assert(dup3(file,43,O_CLOEXEC)==43 && !close(file));
    int sockets[2];assert(!socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,sockets));
    assert(dup2(sockets[1],44)==44 && !close(sockets[1]));
    umask(027);
    assert(!chdir(directory));assert(!setenv("SHOULD_NOT_INHERIT","parent",1));
    struct sigaction action={0};action.sa_handler=caught;sigemptyset(&action.sa_mask);
    assert(!sigaction(SIGUSR1,&action,NULL));action.sa_handler=SIG_IGN;
    assert(!sigaction(SIGUSR2,&action,NULL));
    sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGUSR1);
    assert(!sigprocmask(SIG_BLOCK,&blocked,NULL));assert(!raise(SIGUSR1));
    assert(!pthread_atfork(hook,hook,hook));
    char parent[32];snprintf(parent,sizeof(parent),"%d",getpid());
    char *environment[]={"FRESH_PROBE=exact",NULL};
    for(int i=0;i<4;i++) {
        assert(lseek(42,0,SEEK_SET)==0);
        posix_spawnattr_t attr;assert(!posix_spawnattr_init(&attr));
        sigset_t mask;sigemptyset(&mask);sigaddset(&mask,SIGUSR2);
        assert(!posix_spawnattr_setsigmask(&attr,&mask));
        assert(!posix_spawnattr_setsigdefault(&attr,&mask));
        assert(!posix_spawnattr_setflags(&attr,POSIX_SPAWN_SETSIGDEF|POSIX_SPAWN_SETSIGMASK));
        char *args[]={argv[0],i%2?"explicit":"inherited",parent,directory,NULL};
        assert(write(sockets[0],"ping",4)==4);
        pid_t pid=-1;int error=posix_spawn(&pid,argv[0],NULL,i%2?&attr:NULL,args,environment);
        if(error) fprintf(stderr,"spawn error=%d round=%d\n",error,i);
        assert(!error && pid>0 && pid!=getpid());wait_exit(pid,23);
        char packet[4];assert(read(sockets[0],packet,4)==4 && !memcmp(packet,"pong",4));
        assert(lseek(42,0,SEEK_CUR)==1);assert(!posix_spawnattr_destroy(&attr));
        assert(hooks==0); // POSIX spawn does not run pthread_atfork handlers.
    }
    pid_t nested=fork();assert(nested>=0);
    if(!nested) {
        char *args[]={"/bin/sh","-c","exit 17",NULL};pid_t spawned=-1;
        assert(!posix_spawn(&spawned,args[0],NULL,NULL,args,environment));
        wait_exit(spawned,17);_exit(25);
    }
    wait_exit(nested,25);
    sigset_t pending;assert(!sigpending(&pending) && sigismember(&pending,SIGUSR1));
    assert(!sigaction(SIGUSR1,NULL,&action) && action.sa_handler==caught);
    assert(!sigaction(SIGUSR2,NULL,&action) && action.sa_handler==SIG_IGN);
    char *missing[]={"/no/such/startup6-program",NULL};pid_t pid=-777;
    assert(posix_spawn(&pid,missing[0],NULL,NULL,missing,environment)==ENOENT && pid==-777);
    assert(waitpid(-1,NULL,WNOHANG)==-1 && errno==ECHILD);
    assert(!close(44) && !close(sockets[0]));
    assert(!close(42) && !close(43) && !unlink(path) && !chdir("/") && !rmdir(directory));
    puts("SPAWN_FRESH_OK");return 0;
}
