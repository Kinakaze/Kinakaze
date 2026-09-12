#ifndef KINAKAZE_PTHREAD_H
#define KINAKAZE_PTHREAD_H

#include <kinakaze/types.h>
#include <kinakaze/signal.h>
#include <kinakaze/time.h>

typedef size_t pthread_t;
typedef unsigned int pthread_key_t;
typedef unsigned int pthread_once_t;
typedef struct {
    size_t __stack_size;
    int __detach_state;
    unsigned int __guard_size;
} pthread_attr_t;
typedef struct { size_t __lock; } pthread_mutex_t;
typedef struct { size_t __state; } pthread_cond_t;
/* Registry handle; 0 means "not created yet", which is what the static
   initializer leaves behind and what the first lock resolves. */
typedef struct { size_t __handle; } pthread_rwlock_t;
typedef struct { size_t __handle; } pthread_barrier_t;
typedef struct { size_t __lock; } pthread_spinlock_t;
typedef struct {
    int __kind;
    unsigned int __reserved;
} pthread_mutexattr_t;
typedef struct { size_t __reserved; } pthread_condattr_t;
typedef struct { size_t __reserved; } pthread_rwlockattr_t;
typedef struct { size_t __reserved; } pthread_barrierattr_t;

#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1
#define PTHREAD_ONCE_INIT 0
#define PTHREAD_MUTEX_INITIALIZER { 0 }
#define PTHREAD_COND_INITIALIZER { 0 }
#define PTHREAD_RWLOCK_INITIALIZER { 0 }

#define PTHREAD_MUTEX_NORMAL 0
#define PTHREAD_MUTEX_RECURSIVE 1
#define PTHREAD_MUTEX_ERRORCHECK 2
#define PTHREAD_MUTEX_DEFAULT PTHREAD_MUTEX_NORMAL

/* Returned by pthread_barrier_wait to exactly one thread per generation. */
#define PTHREAD_BARRIER_SERIAL_THREAD (-1)

#define PTHREAD_PROCESS_PRIVATE 0
#define PTHREAD_PROCESS_SHARED 1

#define PTHREAD_CANCEL_ENABLE 0
#define PTHREAD_CANCEL_DISABLE 1
#define PTHREAD_CANCEL_DEFERRED 0
/* Accepted, but behaves as PTHREAD_CANCEL_DEFERRED: stopping a Windows thread
   asynchronously would leave its locks held and its allocations leaked. */
#define PTHREAD_CANCEL_ASYNCHRONOUS 1
#define PTHREAD_CANCELED ((void *) -1)

pthread_t pthread_self(void);
int pthread_equal(pthread_t left, pthread_t right);
int pthread_create(pthread_t *thread, const pthread_attr_t *attr,
                   void *(*start_routine)(void *), void *argument);
int pthread_join(pthread_t thread, void **result);
int pthread_detach(pthread_t thread);
int pthread_attr_init(pthread_attr_t *attr);
int pthread_attr_destroy(pthread_attr_t *attr);
int pthread_attr_setstacksize(pthread_attr_t *attr, size_t stack_size);
int pthread_attr_getstacksize(const pthread_attr_t *attr, size_t *stack_size);
int pthread_attr_setdetachstate(pthread_attr_t *attr, int state);
int pthread_attr_getdetachstate(const pthread_attr_t *attr, int *state);
/* Recorded for round-tripping; Windows installs its own stack guard page. */
int pthread_attr_setguardsize(pthread_attr_t *attr, size_t guard_size);
int pthread_attr_getguardsize(const pthread_attr_t *attr, size_t *guard_size);
int pthread_once(pthread_once_t *control, void (*init_routine)(void));

int pthread_mutex_init(pthread_mutex_t *mutex, const pthread_mutexattr_t *attr);
int pthread_mutex_destroy(pthread_mutex_t *mutex);
int pthread_mutex_lock(pthread_mutex_t *mutex);
int pthread_mutex_trylock(pthread_mutex_t *mutex);
/* Absolute CLOCK_REALTIME deadline; returns ETIMEDOUT once it passes. */
int pthread_mutex_timedlock(pthread_mutex_t *mutex,
                            const struct timespec *deadline);
int pthread_mutex_unlock(pthread_mutex_t *mutex);
int pthread_mutexattr_init(pthread_mutexattr_t *attr);
int pthread_mutexattr_destroy(pthread_mutexattr_t *attr);
int pthread_mutexattr_settype(pthread_mutexattr_t *attr, int kind);
int pthread_mutexattr_gettype(const pthread_mutexattr_t *attr, int *kind);

int pthread_cond_init(pthread_cond_t *cond, const void *attr);
int pthread_cond_destroy(pthread_cond_t *cond);
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex);
/* Absolute CLOCK_REALTIME deadline; on ETIMEDOUT the mutex is reacquired. */
int pthread_cond_timedwait(pthread_cond_t *cond, pthread_mutex_t *mutex,
                           const struct timespec *deadline);
int pthread_cond_signal(pthread_cond_t *cond);
int pthread_cond_broadcast(pthread_cond_t *cond);

int pthread_rwlock_init(pthread_rwlock_t *rwlock,
                        const pthread_rwlockattr_t *attr);
int pthread_rwlock_destroy(pthread_rwlock_t *rwlock);
int pthread_rwlock_rdlock(pthread_rwlock_t *rwlock);
int pthread_rwlock_tryrdlock(pthread_rwlock_t *rwlock);
int pthread_rwlock_wrlock(pthread_rwlock_t *rwlock);
int pthread_rwlock_trywrlock(pthread_rwlock_t *rwlock);
/* Releases whichever mode the calling thread most recently acquired. */
int pthread_rwlock_unlock(pthread_rwlock_t *rwlock);

int pthread_barrier_init(pthread_barrier_t *barrier,
                         const pthread_barrierattr_t *attr, unsigned int count);
int pthread_barrier_destroy(pthread_barrier_t *barrier);
/* PTHREAD_BARRIER_SERIAL_THREAD to one thread of each generation, 0 to the rest. */
int pthread_barrier_wait(pthread_barrier_t *barrier);

int pthread_spin_init(pthread_spinlock_t *lock, int pshared);
int pthread_spin_destroy(pthread_spinlock_t *lock);
int pthread_spin_lock(pthread_spinlock_t *lock);
int pthread_spin_trylock(pthread_spinlock_t *lock);
int pthread_spin_unlock(pthread_spinlock_t *lock);
int pthread_key_create(pthread_key_t *key, void (*destructor)(void *));
int pthread_key_delete(pthread_key_t key);
void *pthread_getspecific(pthread_key_t key);
int pthread_setspecific(pthread_key_t key, const void *value);

/* Deferred cancellation only. pthread_cancel records the request and returns;
   the target thread ends at its next pthread_testcancel and joins as
   PTHREAD_CANCELED. A thread that never calls pthread_testcancel is never
   cancelled, and the cancelled thread's own cleanup does not run. */
int pthread_cancel(pthread_t thread);
int pthread_setcancelstate(int state, int *previous);
int pthread_setcanceltype(int kind, int *previous);
void pthread_testcancel(void);

/* Recorded in registration order but not yet invoked: the hook that would run
   them lives in the fork backend. */
int pthread_atfork(void (*prepare)(void), void (*parent)(void),
                   void (*child)(void));

int pthread_sigmask(int how, const sigset_t *set, sigset_t *old_set);
/* Only self-directed signals are delivered; signal 0 tests for the thread.
   Signalling another thread reports ENOSYS, since per-thread delivery has no
   channel yet. */
int pthread_kill(pthread_t thread, int signal_number);

#endif
