#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <threads.h>
#include <time.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr,"thread compat line %d: %s errno=%d\n",__LINE__,#x,errno); exit(1); } } while (0)
static pthread_mutex_t mutex=PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond=PTHREAD_COND_INITIALIZER;
static pthread_key_t key;
static int destructor_called, selected_cpu, ready;
static void destructor(void *value) { CHECK(value==(void*)0x1234); destructor_called++; }
static void *run(void *unused) {
    (void)unused; CHECK(sched_getcpu()==selected_cpu);
    CHECK(!pthread_setspecific(key,(void*)0x1234)); CHECK(!pthread_mutex_lock(&mutex));
    ready=1; CHECK(!pthread_cond_signal(&cond)); CHECK(!pthread_mutex_unlock(&mutex));
    thrd_exit(-37);
}
static struct timespec deadline(long ns) {
    struct timespec t; CHECK(!clock_gettime(CLOCK_MONOTONIC,&t));
    t.tv_nsec+=ns; t.tv_sec+=t.tv_nsec/1000000000; t.tv_nsec%=1000000000; return t;
}
int main(void) {
    cpu_set_t available, mask, actual; CHECK(!sched_getaffinity(0,sizeof available,&available));
    selected_cpu=-1; for(int i=0;i<64;i++) if(CPU_ISSET(i,&available)) { selected_cpu=i; break; }
    CHECK(selected_cpu>=0); CPU_ZERO(&mask); CPU_SET(selected_cpu,&mask);
    pthread_attr_t attr; CHECK(!pthread_attr_init(&attr));
    CHECK(!pthread_attr_setaffinity_np(&attr,sizeof mask,&mask));
    CHECK(!pthread_attr_getaffinity_np(&attr,sizeof actual,&actual)); CHECK(CPU_EQUAL(&mask,&actual));
    CHECK(!pthread_key_create(&key,destructor)); CHECK(!pthread_mutex_lock(&mutex));
    struct timespec end=deadline(20000000); CHECK(pthread_cond_clockwait(&cond,&mutex,CLOCK_MONOTONIC,&end)==ETIMEDOUT);
    CHECK(pthread_mutex_trylock(&mutex)==EBUSY);
    struct timespec now; CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
    CHECK(now.tv_sec>end.tv_sec || (now.tv_sec==end.tv_sec && now.tv_nsec>=end.tv_nsec));
    end.tv_nsec=1000000000; CHECK(pthread_cond_clockwait(&cond,&mutex,CLOCK_MONOTONIC,&end)==EINVAL);
    end=deadline(100000000); CHECK(pthread_cond_clockwait(&cond,&mutex,CLOCK_PROCESS_CPUTIME_ID,&end)==EINVAL);
    pthread_t thread; CHECK(!pthread_create(&thread,&attr,run,0));
    while(!ready) { end=deadline(500000000); CHECK(!pthread_cond_clockwait(&cond,&mutex,CLOCK_MONOTONIC,&end)); }
    CHECK(!pthread_mutex_unlock(&mutex)); void *result=0; CHECK(!pthread_join(thread,&result));
    CHECK((intptr_t)result==-37 && destructor_called==1);
    CHECK(!pthread_key_delete(key)); CHECK(!pthread_attr_destroy(&attr));
    CHECK(!pthread_cond_destroy(&cond)); CHECK(!pthread_mutex_destroy(&mutex));
    puts("CLOCKWAIT_AFFINITY_THRD_EXIT_TLS_OK"); return 0;
}
