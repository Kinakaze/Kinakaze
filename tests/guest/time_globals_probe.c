/* Exercise the Linux data ABI through both GOT and executable COPY relocations. */
typedef unsigned long size_t;
struct tm { int sec,min,hour,mday,mon,year,wday,yday,isdst; long gmtoff; const char *zone; };
extern long timezone;
extern int daylight;
extern char *tzname[2];
extern void tzset(void);
extern int setenv(const char *,const char *,int), fork(void), waitpid(int,int *,int);
extern struct tm *localtime_r(const long *,struct tm *);
extern long mktime(struct tm *);
extern long write(int,const void *,size_t);
extern void _exit(int) __attribute__((noreturn));
#ifndef COPY_PROBE
extern long __timezone;
extern int __daylight;
extern char *__tzname[2];
extern char **environ, **__environ, **_environ;
#endif
static size_t length(const char *s) { size_t n=0;while(s[n])++n;return n; }
static int equal(const char *a,const char *b) { while(*a && *a==*b){++a;++b;}return *a==*b; }
static __attribute__((noinline)) int same_address(const void *a,const void *b) { return a==b; }
static void require(int ok,const char *error) { if(!ok){write(2,error,length(error));write(2,"\n",1);_exit(91);} }
static void zone(const char *value) { require(!setenv("TZ",value,1),"set TZ"); }
static void check(long west,int dst,const char *standard,const char *summer) {
    require(timezone==west && daylight==dst,"timezone/daylight values");
    require(tzname[0] && tzname[1] && equal(tzname[0],standard) && equal(tzname[1],summer),"tzname pair");
}
__attribute__((noreturn)) void probe_start(void) {
    zone("EST5EDT,M3.2.0,M11.1.0");tzset();check(18000,1,"EST","EDT");
#ifndef COPY_PROBE
    require(same_address(&timezone,&__timezone) && same_address(&daylight,&__daylight) && same_address(tzname,__tzname),"shared timezone aliases");
    require(same_address(&environ,&__environ) && same_address(&environ,&_environ) && environ,"shared environment aliases");
#endif
    long summer=1689422400;
    struct tm local;
    require(localtime_r(&summer,&local) && local.gmtoff==-14400 && local.isdst==1 && equal(local.zone,"EDT"),"summer localtime");
    check(18000,1,"EST","EDT"); /* timezone remains the standard offset in summer. */
    const char *retained=local.zone;
    int pid=fork();require(pid>=0,"fork timezone state");
    if(pid==0) {
        check(18000,1,"EST","EDT");
        require(equal(retained,"EDT"),"inherited tm_zone storage");
        zone("IST-5:30");tzset();check(-19800,0,"IST","IST");
        _exit(0);
    }
    int status=-1;require(waitpid(pid,&status,0)==pid && status==0,"child timezone state");
    check(18000,1,"EST","EDT");
    zone("IST-5:30");require(localtime_r(&summer,&local)!=0,"implicit tzset in localtime");check(-19800,0,"IST","IST");
    zone("UTC0");require(mktime(&local)!=-1,"implicit tzset in mktime");check(0,0,"UTC","UTC");
    require(equal(retained,"EDT"),"old abbreviation lifetime");
    write(1,"TIME_GLOBALS_OK\n",16);_exit(0);
}
__attribute__((naked,noreturn)) void _start(void) { __asm__ volatile("and $-16,%rsp\n\tcall probe_start\n\tud2"); }
