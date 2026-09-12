#ifndef KINAKAZE_EPOLL_H
#define KINAKAZE_EPOLL_H

#include <kinakaze/types.h>

#define EPOLLIN 0x001
#define EPOLLPRI 0x002
#define EPOLLOUT 0x004
#define EPOLLERR 0x008
#define EPOLLHUP 0x010
#define EPOLLRDNORM 0x040
#define EPOLLRDBAND 0x080
#define EPOLLWRNORM 0x100
#define EPOLLWRBAND 0x200
#define EPOLLRDHUP 0x2000
#define EPOLLONESHOT (1u << 30)
#define EPOLLET (1u << 31)

#define EPOLL_CTL_ADD 1
#define EPOLL_CTL_DEL 2
#define EPOLL_CTL_MOD 3

#define EPOLL_CLOEXEC 02000000

typedef union epoll_data {
    void *ptr;
    int fd;
    unsigned int u32;
    unsigned long long u64;
} epoll_data_t;

/*
 * Packed on x86_64: 4 bytes of events then the 8-byte union with no padding
 * between, so the struct is 12 bytes. Guest code indexes arrays of these, so a
 * non-packed layout would misalign every entry past the first.
 */
struct epoll_event {
    unsigned int events;
    epoll_data_t data;
} __attribute__((packed));

#ifdef __cplusplus
extern "C" {
#endif

int epoll_create(int size);
int epoll_create1(int flags);
int epoll_ctl(int epoll_fd, int operation, int fd, struct epoll_event *event);
int epoll_wait(int epoll_fd, struct epoll_event *events, int max_events, int timeout);
int epoll_pwait(int epoll_fd, struct epoll_event *events, int max_events, int timeout,
                const void *mask);

#ifdef __cplusplus
}
#endif

#endif
