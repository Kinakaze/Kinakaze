#define _GNU_SOURCE
#define _LARGEFILE64_SOURCE
#include <errno.h>
#include <ftw.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr,"tree line %d: %s errno=%d\n",__LINE__,#x,errno); exit(1); } } while (0)
static char tree[]="walk-XXXXXX", original[PATH_MAX];
static int mode, count, kinds[7], levels[300], child, forked;
static char visited[64][PATH_MAX];
static int types[64];

static int visit(const char *path, const struct stat *st, int type, struct FTW *info) {
    CHECK(type >= FTW_F && type <= FTW_SLN);
    CHECK(info->level >= 0 && info->level < 300);
    CHECK(info->base == (strrchr(path,'/') ? strrchr(path,'/')+1-path : 0));
    if (mode != 8) {
        CHECK(count < 64); strcpy(visited[count],path); types[count]=type;
    }
    count++; kinds[type]++; levels[info->level]++;
    if (type==FTW_D || type==FTW_DP) CHECK(S_ISDIR(st->st_mode));
    if (type==FTW_SL || type==FTW_SLN) CHECK(S_ISLNK(st->st_mode));
    if (mode==1 || mode==2 || mode==6) {
        char cwd[PATH_MAX], want[PATH_MAX]; CHECK(getcwd(cwd,sizeof cwd));
        CHECK(snprintf(want,sizeof want,"%s/%s",original,path)>0);
        if (type!=FTW_DP) *strrchr(want,'/')=0;
        CHECK(!strcmp(cwd,want));
    }
    if (mode==3 && type==FTW_D && info->level==1) return FTW_SKIP_SUBTREE;
    if (mode==4 && info->level==1) return FTW_SKIP_SIBLINGS;
    if (mode==5) return 37;
    if (mode==6) { errno=EINTR; return -17; }
    if (mode==7 && !forked && type==FTW_D && info->level==1) {
        forked=1; pid_t pid=fork(); CHECK(pid>=0);
        if (!pid) child=1;
        else { int status; CHECK(waitpid(pid,&status,0)==pid); CHECK(WIFEXITED(status) && WEXITSTATUS(status)==0); }
    }
    if (mode==9 && info->level==0) {
        char file[PATH_MAX]; snprintf(file,sizeof file,"%s/loose",tree); CHECK(!unlink(file));
    }
    return 0;
}
static int simple(const char *path,const struct stat *st,int type) {
    (void)path; (void)st; CHECK(type<=FTW_NS); count++; kinds[type]++; return 0;
}
static int simple64(const char *path,const struct stat64 *st,int type) {
    return simple(path,(const struct stat*)st,type);
}
static int detailed64(const char *path,const struct stat64 *st,int type,struct FTW *info) {
    return visit(path,(const struct stat*)st,type,info);
}
static void reset(int selected) {
    mode=selected; count=0; memset(kinds,0,sizeof kinds); memset(levels,0,sizeof levels);
}
static void file(const char *name) {
    FILE *f=fopen(name,"w"); CHECK(f); CHECK(fputs("payload",f)>=0); CHECK(!fclose(f));
}
static void cwd_restored(void) {
    char cwd[PATH_MAX]; CHECK(getcwd(cwd,sizeof cwd)); CHECK(!strcmp(cwd,original));
}
static void counts_physical(void) {
    CHECK(count==10 && kinds[FTW_D]==3 && kinds[FTW_F]==3 && kinds[FTW_SL]==4);
}
int main(void) {
    CHECK(getcwd(original,sizeof original)); CHECK(mkdtemp(tree)); CHECK(!chdir(tree));
    CHECK(!mkdir("a",0700)); CHECK(!mkdir("b",0700)); file("a/one"); file("b/two"); file("loose");
    CHECK(!symlink("loose","file-link")); CHECK(!symlink("a","dir-link"));
    CHECK(!symlink(".","cycle")); CHECK(!symlink("absent","dangling")); CHECK(!chdir(original));
    reset(0); CHECK(!nftw(tree,visit,1,FTW_PHYS)); counts_physical();
    reset(0); CHECK(!nftw64(tree,detailed64,1,FTW_PHYS)); counts_physical();
    reset(0); CHECK(!nftw(tree,visit,0,0)); CHECK(count==8 && kinds[FTW_D]==3 && kinds[FTW_F]==4 && kinds[FTW_SLN]==1);
    reset(0); CHECK(!ftw(tree,simple,1)); CHECK(count==8 && kinds[FTW_NS]==1);
    reset(0); CHECK(!ftw64(tree,simple64,1)); CHECK(count==8 && kinds[FTW_NS]==1);
    reset(1); CHECK(!nftw(tree,visit,1,FTW_PHYS|FTW_CHDIR)); counts_physical(); cwd_restored();
    reset(2); CHECK(!nftw(tree,visit,1,FTW_PHYS|FTW_CHDIR|FTW_DEPTH)); cwd_restored();
    CHECK(count==10 && kinds[FTW_D]==0 && kinds[FTW_DP]==3);
    for(int i=0;i<count;i++) if(types[i]==FTW_DP) {
        size_t n=strlen(visited[i]);
        for(int j=i+1;j<count;j++) CHECK(strncmp(visited[j],visited[i],n) || visited[j][n]!='/');
    }
    reset(3); CHECK(!nftw(tree,visit,1,FTW_PHYS|FTW_ACTIONRETVAL)); CHECK(count==8 && levels[2]==0);
    reset(4); CHECK(!nftw(tree,visit,1,FTW_PHYS|FTW_ACTIONRETVAL)); CHECK(count==2 && levels[1]==1);
    reset(5); CHECK(nftw(tree,visit,1,FTW_PHYS)==37 && count==1);
    reset(6); errno=0; CHECK(nftw(tree,visit,1,FTW_PHYS|FTW_CHDIR)==-17); CHECK(errno==EINTR); cwd_restored();
    reset(7); CHECK(!nftw(tree,visit,1,0)); CHECK(count==8 && kinds[FTW_D]==3 && kinds[FTW_F]==4 && kinds[FTW_SLN]==1);
    CHECK(forked); if(child) _exit(0);
    reset(0); errno=0; CHECK(nftw("missing-root",visit,1,0)==-1 && errno==ENOENT && !count);
    reset(0); CHECK(nftw(tree,visit,1,0x4000)==-1 && errno==EINVAL && !count);
    reset(0); CHECK(!nftw(tree,visit,1,FTW_MOUNT|FTW_PHYS)); counts_physical();
    reset(9); CHECK(!nftw(tree,visit,1,FTW_PHYS)); CHECK(kinds[FTW_NS]==1);
    CHECK(!mkdir("deep",0700)); CHECK(!chdir("deep"));
    for(int i=0;i<128;i++) { CHECK(!mkdir("d",0700)); CHECK(!chdir("d")); }
    CHECK(!chdir(original)); reset(8); CHECK(!nftw("deep",visit,1,FTW_PHYS)); CHECK(count==129 && levels[128]==1);
    puts("FTW_NFTW_LINKS_DEPTH_ACTIONS_CHDIR_FORK_OK"); return 0;
}
