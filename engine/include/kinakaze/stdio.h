#ifndef KINAKAZE_STDIO_H
#define KINAKAZE_STDIO_H

#include <kinakaze/types.h>

#define EOF (-1)
#define BUFSIZ 8192

#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* FILE is opaque: the layout lives entirely inside libc.dll. */
typedef struct FILE FILE;

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The standard streams are variables, as they are in glibc, because compiled
 * code that was built against glibc loads the pointer straight out of the data
 * symbol and never calls an accessor.
 *
 * The accessor functions remain for callers that prefer them. Both name the same
 * three stream objects: two `FILE`s on one descriptor would each buffer part of
 * the output and interleave it.
 */
extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

FILE *kinakaze_stdin(void);
FILE *kinakaze_stdout(void);
FILE *kinakaze_stderr(void);

FILE *fopen(const char *path, const char *mode);
FILE *fdopen(int fd, const char *mode);
int fclose(FILE *file);
int fflush(FILE *file);

size_t fwrite(const void *data, size_t size, size_t count, FILE *file);
size_t fread(void *data, size_t size, size_t count, FILE *file);

int fputc(int character, FILE *file);
int fgetc(FILE *file);
int ungetc(int character, FILE *file);
int fputs(const char *text, FILE *file);
char *fgets(char *buffer, int size, FILE *file);

int putchar(int character);
int puts(const char *text);
int getchar(void);

int printf(const char *format, ...);
int fprintf(FILE *file, const char *format, ...);
int snprintf(char *buffer, size_t size, const char *format, ...);
int sprintf(char *buffer, const char *format, ...);

int feof(FILE *file);
int ferror(FILE *file);
void clearerr(FILE *file);
int fileno(FILE *file);

int fseek(FILE *file, off_t offset, int whence);
off_t ftell(FILE *file);
void rewind(FILE *file);
int setvbuf(FILE *file, char *buffer, int mode, size_t size);

void perror(const char *prefix);
int remove(const char *path);

#ifdef __cplusplus
}
#endif

#endif
