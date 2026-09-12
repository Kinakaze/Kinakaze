#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <printf.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

typedef struct { double number; int integer; } Pair;
static int pair_type, short_modifier, long_modifier, fork_inside, in_child;
static pid_t callback_child;
static FILE *expected_file;
static void pair_argument(void *memory, va_list *ap) { *(Pair *)memory=va_arg(*ap,Pair); }
static int pair_info(const struct printf_info *info,size_t n,int *types,int *sizes) {
    if (!(info->user & (short_modifier|long_modifier))) return -1;
    if (n) { types[0]=pair_type;sizes[0]=sizeof(Pair); }
    return 1;
}
static int pair_print(FILE *stream,const struct printf_info *info,const void *const *args) {
    if(expected_file) assert(stream==expected_file);
    if(fork_inside) {
        fork_inside=0;callback_child=fork();assert(callback_child>=0);
        if(!callback_child) in_child=1;
    }
    const Pair *pair=*(const Pair *const *)args[0];
    assert(info->spec=='P');
    return fprintf(stream,"%c:%d:%.2f:%d:%d:%d",info->user&long_modifier?'L':'S',pair->integer,pair->number,info->width,info->prec,info->left);
}
static int many_info(const struct printf_info *info,size_t n,int *types,int *sizes) {
    (void)info;(void)sizes;const int expected[]={PA_INT,PA_STRING,PA_DOUBLE};
    for(size_t i=0;i<n && i<3;i++)types[i]=expected[i];return 3;
}
static int many_print(FILE *stream,const struct printf_info *info,const void *const *args) {
    (void)info;
    return fprintf(stream,"%d/%s/%.1f",*(const int *)args[0],*(const char *const *)args[1],*(const double *)args[2]);
}
static int zero_info(const struct printf_info *i,size_t n,int *t,int *s) { (void)i;(void)n;(void)t;(void)s;return 0; }
static int error_print(FILE *stream,const struct printf_info *i,const void *const *a) { (void)stream;(void)i;(void)a;errno=EILSEQ;return -1; }
static int decline_info(const struct printf_info *i,size_t n,int *t,int *s) { (void)i;(void)n;(void)t;(void)s;return -1; }
static int impossible(FILE *stream,const struct printf_info *i,const void *const *a) { (void)stream;(void)i;(void)a;abort(); }
static void check(void) {
    char buffer[200];Pair pair={2.5,17};int count=-1;
    int n=snprintf(buffer,sizeof(buffer),"[%*.*ZZP]|%J|%d%n",-12,3,pair,7,"str",1.25,42,&count);
    assert(!strcmp(buffer,"[L:17:2.50:12:3:1]|7/str/1.2|42"));
    assert(n==(int)strlen(buffer) && count==n);
    char small[4];n=snprintf(small,sizeof(small),"%ZP",pair);
    assert(n==(int)strlen("S:17:2.50:0:-1:0") && !strcmp(small,"S:1"));
    assert(snprintf(NULL,0,"%ZP",pair)==n);
    expected_file=tmpfile();assert(expected_file);
    assert(fprintf(expected_file,"prefix:%ZP",pair)>0);rewind(expected_file);
    assert(fgets(buffer,sizeof(buffer),expected_file));assert(!strcmp(buffer,"prefix:S:17:2.50:0:-1:0"));
    assert(!fclose(expected_file));expected_file=NULL;
    int types[8]={-1,-1,-1,-1,-1,-1,-1,-1};
    assert(parse_printf_format("%*.*ZP:%J:%ld",8,types)==7);
    assert(types[0]==PA_INT && types[1]==PA_INT && types[2]==pair_type);
    assert(types[3]==PA_INT && types[4]==PA_STRING && types[5]==PA_DOUBLE && types[6]==(PA_INT|PA_FLAG_LONG));
    assert(parse_printf_format("%ZP",0,NULL)==1);
    errno=0;assert(snprintf(buffer,sizeof(buffer),"%V")==-1 && errno==EILSEQ);
    assert(!register_printf_specifier('f',impossible,decline_info));
    assert(snprintf(buffer,sizeof(buffer),"%.2f:%d",1.25,9)==6 && !strcmp(buffer,"1.25:9"));
    assert(!register_printf_specifier('f',NULL,NULL));
}
int main(void) {
    pair_type=register_printf_type(pair_argument);assert(pair_type>=PA_LAST);
    short_modifier=register_printf_modifier(L"Z");long_modifier=register_printf_modifier(L"ZZ");
    assert(short_modifier>0 && long_modifier>0 && short_modifier!=long_modifier);
    assert(!register_printf_specifier('P',pair_print,pair_info));
    assert(!register_printf_specifier('J',many_print,many_info));
    assert(!register_printf_specifier('V',error_print,zero_info));
    check();pid_t child=fork();assert(child>=0);
    if(!child) {check();_exit(0);}int status;assert(waitpid(child,&status,0)==child && status==0);
    fork_inside=1;char buffer[100];Pair pair={2.5,17};
    assert(snprintf(buffer,sizeof(buffer),"before:%ZP:after",pair)==(int)strlen("before:S:17:2.50:0:-1:0:after"));
    assert(!strcmp(buffer,"before:S:17:2.50:0:-1:0:after"));
    if(in_child)_exit(0);
    assert(waitpid(callback_child,&status,0)==callback_child && status==0);
    errno=0;assert(register_printf_modifier(L"")==-1 && errno==EINVAL);
    errno=0;assert(register_printf_specifier(256,NULL,NULL)==-1 && errno==EINVAL);
    errno=0;assert(register_printf_modifier(L"\u0100")==-1 && errno==EINVAL);
    for(int i=2;i<16;i++)assert(register_printf_modifier(L"unused")==1<<i);
    errno=0;assert(register_printf_modifier(L"last")==-1 && errno==ENOSPC);
    for(int i=pair_type+1;i<256;i++)assert(register_printf_type(pair_argument)==i);
    errno=0;assert(register_printf_type(pair_argument)==-1 && errno==ENOSPC);
    puts("PRINTF_HOOKS_TYPES_MODIFIERS_STREAMS_TRUNCATION_FORK_OK");return 0;
}
