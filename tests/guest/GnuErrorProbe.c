#define _GNU_SOURCE
#include <errno.h>
#include <err.h>
#include <error.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(x) do { if (!(x)) { printf("GNU error line %d: %s errno=%d\n",__LINE__,#x,errno); exit(1); } } while (0)
static int prefixes;
static void prefix(void) { prefixes++; fputs("custom: ",stderr); }
int main(void) {
    FILE *capture=tmpfile(); CHECK(capture); int saved=dup(2); CHECK(saved>=0);
    CHECK(dup2(fileno(capture),2)==2);
    error_message_count=40; error_one_per_line=1; error_print_progname=prefix;
    error(0,0,"number %d",7); CHECK(error_message_count==41 && prefixes==1);
    error_at_line(0,0,"source.c",12,"first"); CHECK(error_message_count==42 && prefixes==2);
    char same[]="source.c";
    error_at_line(0,0,same,12,"suppressed duplicate"); CHECK(error_message_count==42 && prefixes==2);
    warnx("BSD warning"); CHECK(error_message_count==42 && prefixes==2);
    pid_t pid=fork(); CHECK(pid>=0);
    if(!pid) {
        CHECK(error_message_count==42 && error_one_per_line==1 && error_print_progname==prefix);
        error_at_line(0,0,same,12,"still suppressed after fork"); CHECK(error_message_count==42);
        error_at_line(0,0,same,13,"child"); CHECK(error_message_count==43 && prefixes==3); _exit(0);
    }
    int status; CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && !WEXITSTATUS(status));
    CHECK(error_message_count==42 && prefixes==2);
    error_one_per_line=0; error_at_line(0,ENOENT,"source.c",12,"again"); CHECK(error_message_count==43 && prefixes==3);
    CHECK(!fflush(stderr)); CHECK(dup2(saved,2)==2); CHECK(!close(saved));
    rewind(capture); char text[2048]={0}; size_t n=fread(text,1,sizeof text-1,capture); CHECK(n>0); CHECK(!fclose(capture));
    CHECK(strstr(text,"custom: number 7\n")); CHECK(strstr(text,"custom: source.c:12: first\n"));
    CHECK(strstr(text,"custom: source.c:13: child\n")); CHECK(!strstr(text,"suppressed"));
    CHECK(strstr(text,"custom: source.c:12: again: No such file or directory\n"));
    error_print_progname=0; puts("GNU_ERROR_COUNTER_PREFIX_COPY_FORK_OK"); return 0;
}
