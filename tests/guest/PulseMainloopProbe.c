#define _GNU_SOURCE
#include <pulse/mainloop.h>
#include <pulse/thread-mainloop.h>
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

_Static_assert(sizeof(pa_mainloop_api)==112,"Pulse LP64 vtable");
static int io_calls, destroyed, timers, defers, once_calls, replacements, polls;
static pa_defer_event *first_defer,*victim,*survivor;
static struct timeval expected;
static pa_threaded_mainloop *threaded;
static int entered, accepted, thread_events;

static void io_destroy(pa_mainloop_api *a,pa_io_event *e,void *u) { (void)a;(void)e;assert(u==&io_calls);destroyed++; }
static void io_callback(pa_mainloop_api *a,pa_io_event *e,int fd,pa_io_event_flags_t flags,void *u) {
    assert(u==&io_calls && (flags&PA_IO_EVENT_INPUT));
    char c;assert(read(fd,&c,1)==1 && c=='p');io_calls++;
    a->io_enable(e,PA_IO_EVENT_NULL);
}
static void hup_callback(pa_mainloop_api *a,pa_io_event *e,int fd,pa_io_event_flags_t flags,void *u) {
    (void)u;char c;assert(flags&(PA_IO_EVENT_INPUT|PA_IO_EVENT_HANGUP));assert(read(fd,&c,1)==0);
    io_calls++;a->io_free(e);
}
static void timeout_after(struct timeval *tv,int us) {
    assert(!gettimeofday(tv,NULL));tv->tv_usec+=us;
    tv->tv_sec+=tv->tv_usec/1000000;tv->tv_usec%=1000000;
}
static void timer_callback(pa_mainloop_api *a,pa_time_event *e,const struct timeval *tv,void *u) {
    assert(u==&timers && tv->tv_sec==expected.tv_sec && tv->tv_usec==expected.tv_usec);
    timers++;
    if(timers==1) { timeout_after(&expected,20000);a->time_restart(e,&expected); }
}
static void defer_destroy(pa_mainloop_api *a,pa_defer_event *e,void *u) { (void)a;(void)e;(void)u;destroyed++; }
static void replacement(pa_mainloop_api *a,pa_defer_event *e,void *u) { (void)u;replacements++;a->defer_free(e); }
static void defer_callback(pa_mainloop_api *a,pa_defer_event *e,void *u) {
    (void)u;defers++;assert(defers==1);survivor=e;a->defer_enable(e,0);
    a->defer_free(e==first_defer?victim:first_defer);
    assert(a->defer_new(a,replacement,NULL));
}
static void once_callback(pa_mainloop_api *a,void *u) { (void)a;assert(u==&once_calls);once_calls++; }
static int custom_poll(struct pollfd *fds,unsigned long count,int timeout,void *u) {
    assert(u==&polls && count>=1 && timeout==2);polls++;
    return poll(fds,count,timeout);
}
static void quit_callback(pa_mainloop_api *a,void *u) { a->quit(a,*(int *)u); }
static void watchdog(pa_mainloop_api *a,pa_time_event *e,const struct timeval *tv,void *u) {
    (void)a;(void)e;(void)tv;(void)u;assert(!"wakeup did not interrupt poll");
}
static void *wake_thread(void *m) { usleep(30000);pa_mainloop_wakeup(m);return NULL; }
static void threaded_callback(pa_mainloop_api *a,void *u) {
    (void)a;(void)u;assert(pa_threaded_mainloop_in_thread(threaded));
    pa_threaded_mainloop_lock(threaded);entered=1;
    pa_threaded_mainloop_signal(threaded,1);
    accepted=1;pa_threaded_mainloop_unlock(threaded);
    pa_threaded_mainloop_signal(threaded,0);
}
static void threaded_io(pa_mainloop_api *a,pa_io_event *e,int fd,pa_io_event_flags_t flags,void *u) {
    (void)u;assert(pa_threaded_mainloop_in_thread(threaded) && (flags&PA_IO_EVENT_INPUT));
    char c;assert(read(fd,&c,1)==1);thread_events++;a->io_free(e);
    pa_threaded_mainloop_signal(threaded,0);
}
static void fork_refused(void) { errno=0;assert(fork()==-1 && errno==EAGAIN); }
static void fork_clean(void) {
    pid_t child=fork();assert(child>=0);
    if(!child) { pa_mainloop *m=pa_mainloop_new();assert(m);pa_mainloop_free(m);_exit(17); }
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==17);
}
int main(void) {
    pa_mainloop *m=pa_mainloop_new();assert(m);pa_mainloop_api *a=pa_mainloop_get_api(m);
    assert(a && a->userdata && a->io_new && a->time_new && a->defer_new && a->quit);
    assert(pa_mainloop_poll(m)<0 && pa_mainloop_dispatch(m)<0);
    fork_refused();
    int fds[2];assert(!pipe2(fds,O_NONBLOCK|O_CLOEXEC));
    pa_io_event *io=a->io_new(a,fds[0],PA_IO_EVENT_INPUT,io_callback,&io_calls);assert(io);
    a->io_set_destroy(io,io_destroy);
    assert(pa_mainloop_iterate(m,0,NULL)==0);assert(write(fds[1],"p",1)==1);
    assert(pa_mainloop_iterate(m,1,NULL)==1 && io_calls==1);
    assert(write(fds[1],"p",1)==1);assert(pa_mainloop_iterate(m,0,NULL)==0);
    a->io_enable(io,PA_IO_EVENT_INPUT);assert(pa_mainloop_iterate(m,0,NULL)==1 && io_calls==2);
    a->io_free(io);assert(destroyed==1);close(fds[0]);close(fds[1]);
    int sockets[2];assert(!socketpair(AF_UNIX,SOCK_STREAM,0,sockets));
    assert(a->io_new(a,sockets[0],PA_IO_EVENT_INPUT,hup_callback,NULL));close(sockets[1]);
    assert(pa_mainloop_iterate(m,1,NULL)==1 && io_calls==3);close(sockets[0]);
    timeout_after(&expected,20000);
    pa_time_event *timer=a->time_new(a,&expected,timer_callback,&timers);assert(timer);
    while(timers<2)assert(pa_mainloop_iterate(m,1,NULL)>=0);
    for(int i=0;i<3;i++)assert(pa_mainloop_iterate(m,0,NULL)==0);
    assert(timers==2);timeout_after(&expected,10000);a->time_restart(timer,&expected);
    a->time_restart(timer,NULL);assert(pa_mainloop_iterate(m,0,NULL)==0);a->time_free(timer);
    first_defer=a->defer_new(a,defer_callback,NULL);assert(first_defer);a->defer_set_destroy(first_defer,defer_destroy);
    victim=a->defer_new(a,defer_callback,NULL);assert(victim);a->defer_set_destroy(victim,defer_destroy);
    assert(pa_mainloop_iterate(m,0,NULL)==1 && defers==1 && replacements==0 && destroyed==2);
    assert(pa_mainloop_iterate(m,0,NULL)==1 && replacements==1);a->defer_free(survivor);
    pa_mainloop_api_once(a,once_callback,&once_calls);
    assert(pa_mainloop_iterate(m,0,NULL)==1 && once_calls==1);
    assert(pa_mainloop_iterate(m,0,NULL)==0);
    pa_mainloop_set_poll_func(m,custom_poll,&polls);
    pa_mainloop_api_once(a,once_callback,&once_calls);
    assert(pa_mainloop_iterate(m,0,NULL)==1 && once_calls==2 && polls==0);
    assert(!pa_mainloop_prepare(m,1001));assert(pa_mainloop_poll(m)>=0);assert(pa_mainloop_dispatch(m)==0 && polls==1);
    pa_mainloop_set_poll_func(m,NULL,NULL);
    struct timeval limit;timeout_after(&limit,2000000);timer=a->time_new(a,&limit,watchdog,NULL);
    pthread_t thread;assert(!pthread_create(&thread,NULL,wake_thread,m));
    assert(pa_mainloop_iterate(m,1,NULL)==0);assert(!pthread_join(thread,NULL));a->time_free(timer);
    int retval=31,returned=0;pa_mainloop_api_once(a,quit_callback,&retval);
    assert(pa_mainloop_run(m,&returned)>=0 && returned==31 && pa_mainloop_get_retval(m)==31);
    pa_mainloop_free(m);fork_clean();
    threaded=pa_threaded_mainloop_new();assert(threaded);a=pa_threaded_mainloop_get_api(threaded);
    assert(!pa_threaded_mainloop_in_thread(threaded));
    pa_threaded_mainloop_lock(threaded);pa_threaded_mainloop_lock(threaded);
    pa_mainloop_api_once(a,threaded_callback,NULL);assert(!pa_threaded_mainloop_start(threaded));
    while(!entered)pa_threaded_mainloop_wait(threaded);
    assert(!accepted);pa_threaded_mainloop_accept(threaded);
    while(!accepted)pa_threaded_mainloop_wait(threaded);
    assert(!pipe2(fds,O_NONBLOCK|O_CLOEXEC));
    assert(a->io_new(a,fds[0],PA_IO_EVENT_INPUT,threaded_io,NULL));assert(write(fds[1],"x",1)==1);
    while(!thread_events)pa_threaded_mainloop_wait(threaded);
    pa_threaded_mainloop_unlock(threaded);pa_threaded_mainloop_unlock(threaded);
    close(fds[0]);close(fds[1]);fork_refused();
    pa_threaded_mainloop_stop(threaded);
    entered=accepted=0;
    pa_threaded_mainloop_lock(threaded);
    pa_mainloop_api_once(a,threaded_callback,NULL);assert(!pa_threaded_mainloop_start(threaded));
    while(!entered)pa_threaded_mainloop_wait(threaded);
    pa_threaded_mainloop_accept(threaded);
    while(!accepted)pa_threaded_mainloop_wait(threaded);
    pa_threaded_mainloop_unlock(threaded);
    pa_threaded_mainloop_stop(threaded);
    pa_threaded_mainloop_free(threaded);fork_clean();
    puts("PULSE_MAINLOOP_IO_TIMER_DEFER_THREAD_FORK_OK");return 0;
}
