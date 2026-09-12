#ifndef KINAKAZE_UN_H
#define KINAKAZE_UN_H

#include <kinakaze/socket.h>
#include <kinakaze/string.h>

/*
 * struct sockaddr_un is shared with <sys/un.h>. Both headers define it through
 * this guard so the two declarations can never drift apart.
 */
#ifndef KINAKAZE_SOCKADDR_UN_DEFINED
#define KINAKAZE_SOCKADDR_UN_DEFINED
/*
 * 110 bytes on Linux x86_64: the 2-byte family then a fixed 108-byte path with
 * no tail padding, since the whole struct only needs 2-byte alignment. Guest
 * code passes sizeof(struct sockaddr_un) as the socklen_t to bind() and
 * connect(), so the 108 is ABI and not a tunable buffer size.
 */
struct sockaddr_un {
    sa_family_t sun_family;
    char sun_path[108];
};
#endif

/*
 * Length of an abstract-or-filesystem address holding a NUL-terminated path,
 * excluding the terminator. __builtin_offsetof keeps this header free of
 * <stddef.h>, which this tree does not provide.
 */
#define SUN_LEN(ptr) (__builtin_offsetof(struct sockaddr_un, sun_path) + strlen((ptr)->sun_path))

#endif
