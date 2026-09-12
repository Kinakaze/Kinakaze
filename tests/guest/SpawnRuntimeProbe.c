#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

static void wait_exit(pid_t pid,int code) {
    int status;assert(waitpid(pid,&status,0)==pid);
    assert(WIFEXITED(status) && WEXITSTATUS(status)==code);
}
static int child(const char *directory) {
    char cwd[1024],bytes[8]={0};sigset_t mask;
    assert(getcwd(cwd,sizeof(cwd)) && !strcmp(cwd,directory));
    assert(getenv("SPAWN_PROBE") && !strcmp(getenv("SPAWN_PROBE"),"value"));
    assert(getpgrp()==getpid());
    assert(!sigprocmask(SIG_SETMASK,NULL,&mask) && sigismember(&mask,SIGUSR1)==1);
    assert(read(42,bytes,7)==7 && !strcmp(bytes,"payload"));
    assert(!(fcntl(43,F_GETFD)&FD_CLOEXEC));
    assert(fcntl(65535,F_GETFD)==-1 && errno==EBADF);
    assert(write(1,"SPAWN_CHILD_OK",14)==14);return 23;
}
static void launch(const char *program,const char *directory,posix_spawn_file_actions_t *actions,
                   posix_spawnattr_t *attr,int reader,int search) {
    char *argv[]={(char *)program,"--child",(char *)directory,NULL};
    char *environment[]={"SPAWN_PROBE=value",NULL};pid_t pid=-1;
    int error=search?posix_spawnp(&pid,program,actions,attr,argv,environment):
                     posix_spawn(&pid,program,actions,attr,argv,environment);
    assert(error==0 && pid>0);wait_exit(pid,23);
    char bytes[14];size_t received=0;
    while(received<sizeof(bytes)) {
        ssize_t count=read(reader,bytes+received,sizeof(bytes)-received);
        assert(count>0);received+=count;
    }
    assert(!memcmp(bytes,"SPAWN_CHILD_OK",14));
}
int main(int argc,char **argv) {
    if(argc==3 && !strcmp(argv[1],"--child"))return child(argv[2]);
    assert(argc==1);char directory[]="/tmp/spawn-probe-XXXXXX";assert(mkdtemp(directory));
    struct rlimit original,raised={65536,65536};
    assert(!getrlimit(RLIMIT_NOFILE,&original) && !setrlimit(RLIMIT_NOFILE,&raised));
    char filename[128];snprintf(filename,sizeof(filename),"%s/input",directory);
    int file=open(filename,O_CREAT|O_WRONLY|O_TRUNC,0600);assert(file>=0);
    assert(write(file,"payload",7)==7 && !close(file));
    int pipefd[2];assert(!pipe2(pipefd,O_CLOEXEC));
    int kept=open("/dev/null",O_RDONLY|O_CLOEXEC);assert(kept>=0);
    assert(dup3(kept,43,O_CLOEXEC)==43 && !close(kept));
    assert(dup2(43,65535)==65535);
    posix_spawn_file_actions_t actions;posix_spawnattr_t attr;
    assert(!posix_spawn_file_actions_init(&actions) && !posix_spawnattr_init(&attr));
    char copied_path[128];strcpy(copied_path,directory);
    assert(!posix_spawn_file_actions_addchdir_np(&actions,copied_path));
    copied_path[0]='!';
    assert(!posix_spawn_file_actions_addopen(&actions,42,"input",O_RDONLY,0));
    assert(!posix_spawn_file_actions_adddup2(&actions,pipefd[1],1));
    assert(!posix_spawn_file_actions_addclose(&actions,pipefd[0]));
    assert(!posix_spawn_file_actions_adddup2(&actions,43,43));
    assert(!posix_spawn_file_actions_addclosefrom_np(&actions,44));
    sigset_t mask;sigemptyset(&mask);sigaddset(&mask,SIGUSR1);
    assert(!posix_spawnattr_setsigmask(&attr,&mask));
    assert(!posix_spawnattr_setpgroup(&attr,0));
    assert(!posix_spawnattr_setflags(&attr,POSIX_SPAWN_SETSIGMASK|POSIX_SPAWN_SETPGROUP));
    launch(argv[0],directory,&actions,&attr,pipefd[0],0);
    pid_t child_pid=fork();assert(child_pid>=0);
    if(child_pid==0) {
        assert(!posix_spawn_file_actions_addclose(&actions,999));
        launch(argv[0],directory,&actions,&attr,pipefd[0],0);
        assert(!posix_spawn_file_actions_destroy(&actions));_exit(25);
    }
    wait_exit(child_pid,25);
    char path[1024];strcpy(path,argv[0]);char *slash=strrchr(path,'/');assert(slash);
    *slash=0;assert(!setenv("PATH",path,1));
    launch(slash+1,directory,&actions,&attr,pipefd[0],1);
    assert(!posix_spawn_file_actions_destroy(&actions) && !posix_spawnattr_destroy(&attr));
    char *missing[]={"/no/such/kinakaze-spawn-program",NULL};char *empty[]={NULL};pid_t pid=-777;
    assert(posix_spawn(&pid,missing[0],NULL,NULL,missing,empty)==ENOENT && pid==-777);
    assert(!posix_spawn_file_actions_init(&actions));
    assert(!posix_spawn_file_actions_addchdir_np(&actions,missing[0]));
    assert(posix_spawn(&pid,argv[0],&actions,NULL,argv,empty)==ENOENT && pid==-777);
    assert(!posix_spawn_file_actions_destroy(&actions));
    assert(waitpid(-1,NULL,WNOHANG)==-1 && errno==ECHILD);
    assert(!close(pipefd[0]) && !close(pipefd[1]) && !close(43));
    assert(!close(65535) && !setrlimit(RLIMIT_NOFILE,&original));
    assert(!unlink(filename) && !rmdir(directory));puts("SPAWN_RUNTIME_OK");return 0;
}
