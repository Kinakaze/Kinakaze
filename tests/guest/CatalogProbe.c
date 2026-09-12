#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <locale.h>
#include <nl_types.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr,"catalog line %d: %s errno=%d\n",__LINE__,#x,errno); exit(1); } } while (0)
static char fallback[]="caller supplied default";
static void verify(nl_catd cat) {
    CHECK(cat!=(nl_catd)-1);
    CHECK(!strcmp(catgets(cat,1,1,fallback),"first message"));
    CHECK(!strcmp(catgets(cat,1,7,fallback),"escaped\nline"));
    CHECK(!strcmp(catgets(cat,3,1,fallback),"other set"));
    CHECK(!strcmp(catgets(cat,29,79,fallback),"collision search"));
    CHECK(!strcmp(catgets(cat,4,2,fallback),"incremental update"));
    CHECK(!strcmp(catgets(cat,65535,65537,fallback),"wrapped positive hash"));
    CHECK(!strcmp(catgets(cat,65534,65537,fallback),"wrapped negative hash"));
    errno=0; CHECK(catgets(cat,5,999,fallback)==fallback && errno==ENOMSG);
    errno=0; CHECK(catgets(cat,INT_MAX,1,fallback)==fallback && errno==0);
    CHECK(catgets(cat,-1,1,fallback)==fallback);
}
static void malformed(const unsigned char *bytes, size_t size) {
    FILE *f=fopen("bad.cat","wb"); CHECK(f); CHECK(fwrite(bytes,1,size,f)==size); CHECK(!fclose(f));
    errno=0; CHECK(catopen("./bad.cat",0)==(nl_catd)-1 && errno==EINVAL);
}
int main(void) {
    FILE *f=fopen("messages.msg","w"); CHECK(f);
    CHECK(fputs("$set 1\n1 first message\n7 escaped\\nline\n$set 3\n1 other set\n$set 29\n79 collision search\n$set 65535\n65537 wrapped positive hash\n$set 65534\n65537 wrapped negative hash\n",f)>=0);
    CHECK(!fclose(f)); CHECK(system("/usr/bin/gencat messages.cat messages.msg")==0);
    f=fopen("update.msg","w"); CHECK(f); CHECK(fputs("$set 4\n2 incremental update\n",f)>=0); CHECK(!fclose(f));
    CHECK(system("/usr/bin/gencat messages.cat update.msg")==0);
    int failure=system("/usr/bin/gencat invalid.cat nonexistent-input.msg 2> invalid.log");
    CHECK(WIFEXITED(failure) && WEXITSTATUS(failure)!=0);
    nl_catd cat=catopen("./messages.cat",0); verify(cat);
    char *inherited=catgets(cat,1,1,fallback); pid_t pid=fork(); CHECK(pid>=0);
    if(!pid) { verify(cat); CHECK(!strcmp(inherited,"first message")); CHECK(!catclose(cat)); _exit(0); }
    int status; CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && !WEXITSTATUS(status));
    verify(cat); CHECK(!strcmp(inherited,"first message")); CHECK(!catclose(cat));
    CHECK(!setenv("NLSPATH","./missing/%N:./%N.cat",1)); cat=catopen("messages",0); verify(cat); CHECK(!catclose(cat));
    CHECK(!mkdir("en_US.UTF-8",0700)); CHECK(!mkdir("en_US.UTF-8/en-US-UTF-8-%",0700));
    CHECK(!rename("messages.cat","en_US.UTF-8/en-US-UTF-8-%/messages"));
    CHECK(!setenv("LANG","en_US.UTF-8",1)); CHECK(!setenv("NLSPATH","./%L/%l-%t-%c-%%/%N",1));
    cat=catopen("messages",0); verify(cat); CHECK(!catclose(cat));
    CHECK(!rename("en_US.UTF-8/en-US-UTF-8-%/messages","messages.cat"));
    CHECK(!setenv("NLSPATH",":",1)); cat=catopen("messages.cat",0); verify(cat); CHECK(!catclose(cat));
    CHECK(setlocale(LC_MESSAGES,"C")); CHECK(!mkdir("C",0700)); CHECK(!rename("messages.cat","C/messages"));
    CHECK(!setenv("NLSPATH","./%L/%N",1)); cat=catopen("messages",NL_CAT_LOCALE); verify(cat); CHECK(!catclose(cat));
    CHECK(!rename("C/messages","messages.cat"));
    f=fopen("messages.cat","rb"); CHECK(f); unsigned char bytes[4096]; size_t size=fread(bytes,1,sizeof bytes,f); CHECK(!fclose(f)); CHECK(size>36);
    malformed(bytes,4); malformed(bytes,20);
    unsigned char save[12]; memcpy(save,bytes,12);
    // gencat accepts either header endian; tables retain their fixed LE/BE order.
    for(int i=0;i<12;i+=4) { unsigned char a=bytes[i], b=bytes[i+1]; bytes[i]=bytes[i+3]; bytes[i+1]=bytes[i+2]; bytes[i+2]=b; bytes[i+3]=a; }
    f=fopen("swapped.cat","wb"); CHECK(f); CHECK(fwrite(bytes,1,size,f)==size); CHECK(!fclose(f));
    cat=catopen("./swapped.cat",0); verify(cat); CHECK(!catclose(cat)); memcpy(bytes,save,12);
    memset(bytes+4,255,8); malformed(bytes,size); memcpy(bytes,save,12);
    uint32_t width,depth; memcpy(&width,bytes+4,4); memcpy(&depth,bytes+8,4);
    size_t strings=12+(size_t)width*depth*24; CHECK(strings<size);
    for(size_t i=strings;i<size;i++) if(bytes[i]==0) bytes[i]='X'; malformed(bytes,size);
    errno=0; CHECK(catgets((nl_catd)-1,1,1,fallback)==fallback && !errno);
    CHECK(catclose((nl_catd)-1)==-1 && errno==EBADF);
    errno=0; CHECK(catopen("./not-present.cat",0)==(nl_catd)-1 && errno==ENOENT);
    puts("GENCAT_CATOPEN_CATGETS_SEARCH_ENDIAN_FORK_OK"); return 0;
}
