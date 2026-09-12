#ifndef KINAKAZE_SOCKET_H
#define KINAKAZE_SOCKET_H

#include <kinakaze/types.h>

typedef unsigned short sa_family_t;
typedef unsigned int socklen_t;
typedef unsigned short in_port_t;
typedef unsigned int in_addr_t;

/* Linux x86_64 address families. AF_INET6 is 10 here, not the Windows 23. */
#define AF_UNSPEC 0
#define AF_UNIX 1
#define AF_LOCAL 1
#define AF_INET 2
#define AF_INET6 10

#define PF_UNSPEC AF_UNSPEC
#define PF_INET AF_INET
#define PF_INET6 AF_INET6

#define SOCK_STREAM 1
#define SOCK_DGRAM 2
#define SOCK_RAW 3
#define SOCK_SEQPACKET 5
/* Linux ORs these into the type argument of socket() and accept4(). */
#define SOCK_NONBLOCK 04000
#define SOCK_CLOEXEC 02000000

#define IPPROTO_IP 0
#define IPPROTO_TCP 6
#define IPPROTO_UDP 17
#define IPPROTO_IPV6 41

/* Linux SOL_SOCKET is 1; Windows spells it 65535. */
#define SOL_SOCKET 1

#define SO_DEBUG 1
#define SO_REUSEADDR 2
#define SO_TYPE 3
#define SO_ERROR 4
#define SO_DONTROUTE 5
#define SO_BROADCAST 6
#define SO_SNDBUF 7
#define SO_RCVBUF 8
#define SO_KEEPALIVE 9
#define SO_OOBINLINE 10
#define SO_LINGER 13
#define SO_REUSEPORT 15
#define SO_PEERCRED 17
#define SO_RCVTIMEO 20
#define SO_SNDTIMEO 21
#define SO_ACCEPTCONN 30

#define TCP_NODELAY 1

#define MSG_OOB 0x01
#define MSG_PEEK 0x02
#define MSG_DONTROUTE 0x04
#define MSG_DONTWAIT 0x40
#define MSG_WAITALL 0x100
#define MSG_NOSIGNAL 0x4000

#define SHUT_RD 0
#define SHUT_WR 1
#define SHUT_RDWR 2

#define INADDR_ANY ((in_addr_t)0x00000000)
#define INADDR_LOOPBACK ((in_addr_t)0x7f000001)
#define INADDR_BROADCAST ((in_addr_t)0xffffffff)

/* The generic address, 16 bytes as on Linux. */
struct sockaddr {
    sa_family_t sa_family;
    char sa_data[14];
};

/* Large enough for any supported address, matching Linux at 128 bytes. */
struct sockaddr_storage {
    sa_family_t ss_family;
    char __padding[126];
};

struct in_addr {
    in_addr_t s_addr;
};

struct sockaddr_in {
    sa_family_t sin_family;
    in_port_t sin_port;
    struct in_addr sin_addr;
    unsigned char sin_zero[8];
};

struct in6_addr {
    unsigned char s6_addr[16];
};

struct sockaddr_in6 {
    sa_family_t sin6_family;
    in_port_t sin6_port;
    unsigned int sin6_flowinfo;
    struct in6_addr sin6_addr;
    unsigned int sin6_scope_id;
};

struct linger {
    int l_onoff;
    int l_linger;
};

/*
 * The payload of SO_PEERCRED on an AF_UNIX socket. Guarded because <sys/un.h>
 * users may reach this type through either header.
 */
#ifndef KINAKAZE_UCRED_DEFINED
#define KINAKAZE_UCRED_DEFINED
struct ucred {
    pid_t pid;
    uid_t uid;
    gid_t gid;
};
#endif

#ifdef __cplusplus
extern "C" {
#endif

int socket(int family, int type, int protocol);
int socketpair(int domain, int type, int protocol, int sv[2]);
int bind(int fd, const struct sockaddr *address, socklen_t length);
int listen(int fd, int backlog);
int accept(int fd, struct sockaddr *address, socklen_t *length);
int accept4(int fd, struct sockaddr *address, socklen_t *length, int flags);
int connect(int fd, const struct sockaddr *address, socklen_t length);

ssize_t send(int fd, const void *buffer, size_t length, int flags);
ssize_t recv(int fd, void *buffer, size_t length, int flags);
ssize_t sendto(int fd, const void *buffer, size_t length, int flags,
               const struct sockaddr *address, socklen_t address_length);
ssize_t recvfrom(int fd, void *buffer, size_t length, int flags,
                 struct sockaddr *address, socklen_t *address_length);

int shutdown(int fd, int how);
int setsockopt(int fd, int level, int name, const void *value, socklen_t length);
int getsockopt(int fd, int level, int name, void *value, socklen_t *length);
int getsockname(int fd, struct sockaddr *address, socklen_t *length);
int getpeername(int fd, struct sockaddr *address, socklen_t *length);

unsigned short htons(unsigned short value);
unsigned short ntohs(unsigned short value);
unsigned int htonl(unsigned int value);
unsigned int ntohl(unsigned int value);

#ifdef __cplusplus
}
#endif

#endif
