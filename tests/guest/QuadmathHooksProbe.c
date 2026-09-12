#define _GNU_SOURCE
#include <assert.h>
#include <quadmath.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
static void check(void) {
    char buffer[256], reference[256];__float128 value=1.0Q/3.0Q;
    int n=snprintf(buffer,sizeof(buffer),"%.30Qg",value);
    assert(n==32 && !strcmp(buffer,"0.333333333333333333333333333333"));
    assert(quadmath_snprintf(reference,sizeof(reference),"%+#40.24Qe",value)>0);
    assert(snprintf(buffer,sizeof(buffer),"%+#40.24Qe",value)==(int)strlen(reference));
    assert(!strcmp(buffer,reference));
    char short_buffer[4];assert(snprintf(short_buffer,sizeof(short_buffer),"%.30Qg",value)==32);
    assert(!strcmp(short_buffer,"0.3"));
    assert(snprintf(buffer,sizeof(buffer),"%.2f/%d/%.3Qg",1.25,7,value)==12);
    assert(!strcmp(buffer,"1.25/7/0.333"));
}
int main(void) {
    check();pid_t child=fork();assert(child>=0);
    if(!child){check();_exit(0);}int status;assert(waitpid(child,&status,0)==child && status==0);
    puts("QUADMATH_PRINTF_BINARY128_FORK_OK");return 0;
}
