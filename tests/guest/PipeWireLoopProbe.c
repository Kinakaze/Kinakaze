
typedef unsigned long size_t;
typedef unsigned long long u64;
typedef _Bool bool;
#define NULL ((void*)0)
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
struct iface { const char *type; unsigned version; const void *methods; void *data; };
struct loop { struct iface *system, *loop, *control, *utils; const char *name; };
struct source { struct iface *loop; void (*func)(struct source*); void *data; int fd; unsigned mask, rmask; void *priv; };
struct timespec { long sec, nsec; };
typedef int (*invoke_fn)(struct iface*,bool,unsigned,const void*,size_t,void*);
struct methods {
 unsigned version;
 int (*add)(void*,struct source*); int (*update)(void*,struct source*); int (*remove)(void*,struct source*);
 int (*invoke)(void*,invoke_fn,unsigned,const void*,size_t,bool,void*);
};
struct hook { void *next, *prev, *funcs, *data, *removed, *priv; };
struct hooks { unsigned version; void (*before)(void*); void (*after)(void*); };
struct control {
 unsigned version; int (*get_fd)(void*); void (*add_hook)(void*,struct hook*,const struct hooks*,void*);
 void (*enter)(void*); void (*leave)(void*); int (*iterate)(void*,int); int (*check)(void*);
};
struct utils {
 unsigned version;
 struct source* (*add_io)(void*,int,unsigned,bool,void(*)(void*,int,unsigned),void*);
 int (*update_io)(void*,struct source*,unsigned);
 struct source* (*add_idle)(void*,bool,void(*)(void*),void*);
 int (*enable_idle)(void*,struct source*,bool);
 struct source* (*add_event)(void*,void(*)(void*,u64),void*);
 int (*signal_event)(void*,struct source*);
 struct source* (*add_timer)(void*,void(*)(void*,u64),void*);
 int (*update_timer)(void*,struct source*,const struct timespec*,const struct timespec*,bool);
 struct source* (*add_signal)(void*,int,void(*)(void*,int),void*);
 void (*destroy)(void*,struct source*);
};
extern void *pw_main_loop_new(const void*);
extern struct loop *pw_main_loop_get_loop(void*);
extern int pw_main_loop_run(void*), pw_main_loop_quit(void*);
extern void pw_main_loop_destroy(void*);
extern void *pw_thread_loop_new(const char*,const void*);
extern struct loop *pw_thread_loop_get_loop(void*);
extern int pw_thread_loop_start(void*);
extern void pw_thread_loop_stop(void*), pw_thread_loop_destroy(void*);
extern void pw_thread_loop_lock(void*), pw_thread_loop_unlock(void*);
extern int pipe2(int*,int), close(int), fcntl(int,int,...), clock_gettime(int,struct timespec*);
extern long read(int,void*,size_t), write(int,const void*,size_t);
struct pollfd { int fd; short events,revents; };
extern int poll(struct pollfd*,unsigned long,int);
extern int pthread_create(unsigned long*,const void*,void*(*)(void*),void*), pthread_join(unsigned long,void**);
static u64 event_count, timer_count;
static int idle_count, io_count, raw_count, hook_stage, hook_fail, calls, async_seen;
static void counted(void *p,u64 n) { *(u64*)p += n; }
static void idle(void *p) { ++*(int*)p; }
static void io(void *p,int fd,unsigned mask) { char b; if ((mask&1) && read(fd,&b,1)==1 && b=='Q') ++*(int*)p; }
static void raw(struct source *s) { io(s->data,s->fd,s->rmask); }
static void before(void *p) { if (hook_stage!=0) hook_fail=1; hook_stage=1; }
static void after(void *p) { if (hook_stage!=1) hook_fail=1; hook_stage=0; }
static int invoked(struct iface *loop,bool async,unsigned seq,const void *bytes,size_t size,void *p) {
 if (size!=4 || *(const unsigned*)bytes!=0x12345678 || seq!=73) return -123;
 ++calls; async_seen=async; return 91;
}
static double now(void) { struct timespec t; clock_gettime(1,&t); return t.sec*1000.0+t.nsec/1000000.0; }
static void *run(void *main) { return (void*)(long)pw_main_loop_run(main); }
static void quit_timer(void *main,u64 n) { ++timer_count; pw_main_loop_quit(main); }
int probe(double *idle_ms) {
 void *main=pw_main_loop_new(NULL); CHECK(main);
 struct loop *l=pw_main_loop_get_loop(main); CHECK(l && l->name && l->system && l->loop && l->control && l->utils);
 const struct control *c=l->control->methods; const struct utils *u=l->utils->methods; const struct methods *m=l->loop->methods;
 void *cd=l->control->data,*ud=l->utils->data,*md=l->loop->data;
 CHECK(c->get_fd(cd)>=0 && c->check(cd)==0); c->enter(cd); CHECK(c->check(cd)==1);
 struct hook hook={0}; const struct hooks hooks={0,before,after}; c->add_hook(cd,&hook,&hooks,NULL);
 struct source *e=u->add_event(ud,counted,&event_count); CHECK(e);
 CHECK(c->iterate(cd,0)==0);
 struct pollfd public_fd={c->get_fd(cd),1,0}; CHECK(poll(&public_fd,1,0)==0);
 CHECK(u->signal_event(ud,e)==0 && u->signal_event(ud,e)==0 && u->signal_event(ud,e)==0);
 CHECK(poll(&public_fd,1,0)==1 && public_fd.revents==1);
 CHECK(c->iterate(cd,0)>=0 && event_count==3);
 CHECK(poll(&public_fd,1,0)==0);
 CHECK(c->iterate(cd,0)==0 && event_count==3);
 struct source *i=u->add_idle(ud,1,idle,&idle_count); CHECK(i);
 CHECK(c->iterate(cd,0)>=1 && c->iterate(cd,0)>=1 && idle_count==2);
 CHECK(u->enable_idle(ud,i,0)==0 && c->iterate(cd,0)==0);
 double start=now(); CHECK(c->iterate(cd,25)==0); *idle_ms=now()-start; CHECK(*idle_ms>=15 && *idle_ms<2000);
 int fds[2]; CHECK(pipe2(fds,0x80800)==0);
 struct source *s=u->add_io(ud,fds[0],1,1,io,&io_count); CHECK(s);
 CHECK(write(fds[1],"Q",1)==1 && c->iterate(cd,50)>=1 && io_count==1);
 CHECK(u->update_io(ud,s,0)==0); CHECK(write(fds[1],"Q",1)==1 && c->iterate(cd,0)==0);
 CHECK(u->update_io(ud,s,1)==0 && c->iterate(cd,0)>=1 && io_count==2);
 u->destroy(ud,s); CHECK(fcntl(fds[0],1)==-1); close(fds[1]);
 CHECK(pipe2(fds,0x80800)==0);
 struct source caller={0,raw,&raw_count,fds[0],1,0,0}; CHECK(m->add(md,&caller)==0);
 CHECK(write(fds[1],"Q",1)==1 && c->iterate(cd,50)>=1 && raw_count==1);
 CHECK(m->remove(md,&caller)==0 && !caller.loop && fcntl(fds[0],1)>=0); close(fds[0]);close(fds[1]);
 struct source *timer=u->add_timer(ud,counted,&timer_count); CHECK(timer);
 struct timespec deadline={0,20000000}; CHECK(u->update_timer(ud,timer,&deadline,NULL,0)==0);
 start=now(); while (!timer_count && now()-start<2000) CHECK(c->iterate(cd,50)>=0);
 CHECK(timer_count==1); CHECK(u->update_timer(ud,timer,NULL,NULL,0)==0);
 CHECK(c->iterate(cd,0)==0 && !hook_fail && hook_stage==0);
 unsigned payload=0x12345678; CHECK(m->invoke(md,invoked,73,&payload,4,1,NULL)==91 && calls==1 && !async_seen);
 c->leave(cd); CHECK(c->check(cd)==0);
 CHECK(m->invoke(md,invoked,73,&payload,4,0,NULL)==(73|0x40000000)); payload=0;
 c->enter(cd); CHECK(c->iterate(cd,0)>=1 && calls==2 && async_seen); c->leave(cd);
 // Both source destruction and loop destruction close owned fds.
 int event_fd=e->fd; u->destroy(ud,i); u->destroy(ud,timer);
 pw_main_loop_destroy(main); CHECK(fcntl(event_fd,1)==-1 && hook.next==&hook && hook.prev==&hook);
 main=pw_main_loop_new(NULL); CHECK(main); l=pw_main_loop_get_loop(main); u=l->utils->methods; ud=l->utils->data;
 timer=u->add_timer(ud,quit_timer,main); CHECK(timer); CHECK(u->update_timer(ud,timer,&deadline,NULL,0)==0);
 CHECK(pw_main_loop_run(main)==0); pw_main_loop_destroy(main);
 // Invoke from a second thread must wake a parked loop and copy the payload.
 main=pw_main_loop_new(NULL); CHECK(main); l=pw_main_loop_get_loop(main); m=l->loop->methods; md=l->loop->data;
 unsigned long thread; CHECK(pthread_create(&thread,NULL,run,main)==0); payload=0x12345678;
 CHECK(m->invoke(md,invoked,73,&payload,4,1,NULL)==91 && calls==3 && async_seen);
 CHECK(pw_main_loop_quit(main)==0 && pthread_join(thread,NULL)==0); pw_main_loop_destroy(main);
 // Thread-loop callbacks share its recursive lock, and stop can wake a worker
 // while the caller retains that lock (no join/callback deadlock).
 void *audio=pw_thread_loop_new("probe",NULL); CHECK(audio); l=pw_thread_loop_get_loop(audio); m=l->loop->methods; md=l->loop->data;
 CHECK(pw_thread_loop_start(audio)==0); CHECK(m->invoke(md,invoked,73,&payload,4,1,NULL)==91 && calls==4);
 pw_thread_loop_lock(audio); pw_thread_loop_stop(audio); pw_thread_loop_unlock(audio);
 CHECK(pw_thread_loop_start(audio)==0); CHECK(m->invoke(md,invoked,73,&payload,4,1,NULL)==91 && calls==5);
 pw_thread_loop_destroy(audio);
 return 0;
}
