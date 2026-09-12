#ifndef KINAKAZE_STRING_H
#define KINAKAZE_STRING_H

#include <kinakaze/types.h>

#ifdef __cplusplus
extern "C" {
#endif

size_t strlen(const char *text);
size_t strnlen(const char *text, size_t limit);

void *memcpy(void *destination, const void *source, size_t count);
void *memmove(void *destination, const void *source, size_t count);
void *memset(void *destination, int value, size_t count);
int memcmp(const void *left, const void *right, size_t count);
void *memchr(const void *haystack, int needle, size_t count);

char *strcpy(char *destination, const char *source);
char *strncpy(char *destination, const char *source, size_t limit);
char *strcat(char *destination, const char *source);
char *strncat(char *destination, const char *source, size_t limit);

int strcmp(const char *left, const char *right);
int strncmp(const char *left, const char *right, size_t limit);

char *strchr(const char *text, int needle);
char *strrchr(const char *text, int needle);
char *strstr(const char *haystack, const char *needle);
size_t strspn(const char *text, const char *accept);
size_t strcspn(const char *text, const char *reject);
char *strpbrk(const char *text, const char *accept);

char *strdup(const char *text);
char *strndup(const char *text, size_t limit);

char *strerror(int error);
int strerror_r(int error, char *buffer, size_t size);

#ifdef __cplusplus
}
#endif

#endif
