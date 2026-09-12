#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
int main(void) {
    const size_t sizes[]={8,32,64,128,256};
    const int32_t first[]={1103527590,964237963,1894937090,1804289383,510644794};
    for(int k=0;k<5;k++) {
        struct random_data data={0},zero={0};char state[257],second[257];int32_t x,y;
        memset(state,0xa5,sizeof(state));memset(second,0xa5,sizeof(second));
        assert(!initstate_r(1,state+1,sizes[k],&data));
        assert(!initstate_r(0,second+1,sizes[k],&zero));
        assert(!random_r(&data,&x));if(x!=first[k]){char b[100];int n=snprintf(b,sizeof(b),"state size %zu: got %d expected %d\n",sizes[k],x,first[k]);write(2,b,n);}assert(x==first[k]);assert(!random_r(&zero,&y) && x==y);
        for(int i=0;i<1000;i++) {assert(!random_r(&data,&x));assert(!random_r(&zero,&y));assert(x==y && x>=0);}
        assert((unsigned char)state[0]==0xa5);
        assert(!srandom_r(UINT32_MAX,&data));assert(!srandom_r(UINT32_MAX,&zero));
        pid_t child=fork();assert(child>=0);
        if(!child) {for(int i=0;i<300;i++){assert(!random_r(&data,&x));assert(!random_r(&zero,&y) && x==y);}_exit(0);}
        int status;assert(waitpid(child,&status,0)==child && status==0);
        for(int i=0;i<300;i++){assert(!random_r(&data,&x));assert(!random_r(&zero,&y) && x==y);}
        char replacement[256];assert(!initstate_r(9,replacement,sizeof(replacement),&data));
        assert(!setstate_r(state+1,&data));assert(!random_r(&data,&x));assert(!random_r(&zero,&y) && x==y);
        assert(!setstate_r(state+1,&data));assert(!random_r(&data,&x));assert(!random_r(&zero,&y) && x==y);
    }
    struct random_data data={0};char state[8];int32_t value;
    errno=0;assert(initstate_r(1,state,7,&data)==-1 && errno==EINVAL);
    errno=0;assert(random_r(NULL,&value)==-1 && errno==EINVAL);
    puts("RANDOM_STATE_ALL_TYPES_UNALIGNED_SWITCH_FORK_OK");return 0;
}
