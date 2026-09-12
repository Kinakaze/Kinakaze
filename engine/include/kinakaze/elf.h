#ifndef KINAKAZE_ELF_H
#define KINAKAZE_ELF_H

#include <kinakaze/types.h>

typedef struct kinakaze_tls_index {
    size_t module;
    size_t offset;
} kinakaze_tls_index;

void *__tls_get_addr(const kinakaze_tls_index *index);

#endif
