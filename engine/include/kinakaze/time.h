#ifndef KINAKAZE_TIME_H
#define KINAKAZE_TIME_H

#ifndef KINAKAZE_TIME_T_DEFINED
#define KINAKAZE_TIME_T_DEFINED
typedef long time_t;
#endif

typedef int clockid_t;

struct timespec {
    time_t tv_sec;
    long tv_nsec;
};

#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1

int clock_gettime(clockid_t clock, struct timespec *result);

#endif

