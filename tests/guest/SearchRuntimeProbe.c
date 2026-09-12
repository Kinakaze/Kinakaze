#define _GNU_SOURCE
#include <assert.h>
#include <search.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <stdint.h>
#include <error.h>
#include <argz.h>
#include <argp.h>
#include <sys/wait.h>
#include <unistd.h>

enum {N=4096};
static unsigned comparisons,visits,frees;
static int previous=-1;
static int compare(const void *a,const void *b){
    comparisons++;return (*(const int *)a>*(const int *)b)-(*(const int *)a<*(const int *)b);
}
static void visit(const void *node,VISIT order,int level){
    assert(level<24);
    if(order==postorder || order==leaf){int value=**(int *const *)node;assert(value>previous);previous=value;visits++;}
}
static void visit_r(const void *node,VISIT order,void *context){
    if(order==postorder || order==leaf){assert(**(int *const *)node>=0);(*(unsigned *)context)++;}
}
static void release(void *key){frees++;free(key);}
static unsigned parsed_args,parser_end,parser_fini,parser_error;
static error_t parse_argument(int key,char *arg,struct argp_state *state){
    if(key==ARGP_KEY_ARG){
        if(!strcmp(arg,"unclaimed"))return ARGP_ERR_UNKNOWN;
        assert(state->next>0 && state->argv[state->next-1]==arg);parsed_args++;return 0;
    }
    if(key==ARGP_KEY_END)parser_end++;
    if(key==ARGP_KEY_FINI)parser_fini++;
    if(key==ARGP_KEY_ERROR)parser_error++;
    return ARGP_ERR_UNKNOWN;
}
int main(void){
    struct argp parser={.parser=parse_argument};int next=-1;
    char *unclaimed[]={"probe","unclaimed",NULL};
    assert(!argp_parse(&parser,2,unclaimed,ARGP_SILENT,&next,NULL) && next==1);
    assert(!parsed_args && !parser_end && parser_fini==1);
    assert(argp_parse(&parser,2,unclaimed,ARGP_SILENT,NULL,NULL)==EINVAL && parser_error==1);
    char *quoted[]={"probe","--","-literal",NULL};
    assert(!argp_parse(&parser,3,quoted,ARGP_SILENT,&next,NULL) && next==3);
    assert(parsed_args==1 && parser_end==1 && parser_fini==3);
    char *ignored[]={"probe","accepted",NULL};
    assert(!argp_parse(&parser,2,ignored,ARGP_SILENT|ARGP_NO_ARGS,&next,NULL) && next==1 && parsed_args==1);
    char *packed=NULL;size_t packed_length=0;
    assert(!argz_create_sep("::a:::b::",':',&packed,&packed_length));
    assert(packed_length==5 && !memcmp(packed,"a\0b\0\0",5));free(packed);
    assert(!argz_create_sep("",':',&packed,&packed_length) && !packed && !packed_length);
    assert(!argz_create_sep("::",':',&packed,&packed_length) && packed_length==1 && !*packed);free(packed);
    assert(!argz_create_sep("a:b",0,&packed,&packed_length) && packed_length==4 && !strcmp(packed,"a:b"));free(packed);
    assert(!argz_create_sep("a\xff""b",-1,&packed,&packed_length) && packed_length==4 && !memcmp(packed,"a\0b\0",4));free(packed);
    int errors_pipe[2];assert(!pipe(errors_pipe));pid_t diagnostic=fork();assert(diagnostic>=0);
    if(!diagnostic){
        close(errors_pipe[0]);assert(dup2(errors_pipe[1],2)==2);close(errors_pipe[1]);
        error_at_line(23,0,"probe.c",17,"%s %d %lld %.1f","value",42,1LL<<40,2.5);_exit(99);
    }
    close(errors_pipe[1]);char message[512];size_t used=0;ssize_t got;
    while((got=read(errors_pipe[0],message+used,sizeof message-1-used))>0)used+=got;
    assert(got==0);message[used]=0;close(errors_pipe[0]);int diagnostic_status;
    assert(waitpid(diagnostic,&diagnostic_status,0)==diagnostic && WIFEXITED(diagnostic_status) && WEXITSTATUS(diagnostic_status)==23);
    assert(strstr(message,"probe.c:17: value 42 1099511627776 2.5\n"));
    struct hsearch_data table={0};assert(hcreate_r(64,&table));
    char keys[64][16];ENTRY *first=NULL;
    for(int i=0;i<64;i++){
        snprintf(keys[i],sizeof keys[i],"key-%d",i);ENTRY entry={keys[i],(void *)(uintptr_t)(i+1)},*out=NULL;
        assert(hsearch_r(entry,ENTER,&out,&table) && out->data==entry.data);
        if(i==0)first=out;
    }
    ENTRY duplicate={keys[0],NULL},*out=NULL;
    assert(hsearch_r(duplicate,ENTER,&out,&table) && out==first && out->data==(void *)1);
    ENTRY missing={"absent",NULL};errno=0;
    assert(!hsearch_r(missing,FIND,&out,&table) && !out && errno==ESRCH);
    pid_t hash_child=fork();assert(hash_child>=0);
    if(!hash_child){assert(hsearch_r(duplicate,FIND,&out,&table) && out==first);hdestroy_r(&table);_exit(0);}
    int hash_status;assert(waitpid(hash_child,&hash_status,0)==hash_child && hash_status==0);
    for(int i=0;i<64;i++){ENTRY entry={keys[i],NULL};assert(hsearch_r(entry,FIND,&out,&table) && out->data==(void *)(uintptr_t)(i+1));}
    hdestroy_r(&table);assert(!table.table);
    char *end=NULL;errno=0;
    assert(strtoull("d168fd5efc2d2b72!",&end,16)==UINT64_C(0xd168fd5efc2d2b72) && *end=='!' && !errno);
    assert(strtol("-9223372036854775808",NULL,10)==INT64_MIN && !errno);
    for(int permutation=0;permutation<3;permutation++){
        void *root=NULL;comparisons=0;
        for(int i=0;i<N;i++){
            int *key=malloc(sizeof *key);assert(key);
            *key=permutation==0?i:permutation==1?N-1-i:(i*1543)&(N-1);
            void *node=tsearch(key,&root,compare);assert(node && *(int **)node==key);
        }
        assert(comparisons<N*24);previous=-1;visits=0;twalk(root,visit);assert(visits==N);
        unsigned count=0;twalk_r(root,visit_r,&count);assert(count==N);
        for(int i=0;i<N;i++){void *node=tfind(&i,&root,compare);assert(node && **(int **)node==i);assert(tsearch(&i,&root,compare)==node);}
        pid_t child=fork();assert(child>=0);
        if(!child){frees=0;tdestroy(root,release);assert(frees==N);_exit(0);}
        int status;assert(waitpid(child,&status,0)==child && status==0);
        for(int i=0;i<N;i+=2){void *node=tfind(&i,&root,compare);int *key=*(int **)node;assert(tdelete(&i,&root,compare));free(key);assert(!tfind(&i,&root,compare));}
        previous=-1;visits=0;twalk(root,visit);assert(visits==N/2);
        for(int i=N-1;i>=0;i-=2){void *node=tfind(&i,&root,compare);int *key=*(int **)node;assert(tdelete(&i,&root,compare));free(key);}
        assert(!root);assert(!tdelete(&status,&root,compare));
    }
    puts("SEARCH_TREE_FORK_OK");return 0;
}
