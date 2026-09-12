typedef unsigned long size_t;
typedef long ssize_t;
extern void *memcpy(void *, const void *, size_t);
extern void *memset(void *, int, size_t);
extern int memcmp(const void *, const void *, size_t);
extern void free(void *);
extern void *malloc(size_t);
struct Functions {
    ssize_t (*read)(void *, char *, size_t);
    ssize_t (*write)(void *, const char *, size_t);
    int (*seek)(void *, long *, int);
    int (*close)(void *);
};
extern void *fopencookie(void *, const char *, struct Functions);
extern void *fopen(const char *, const char *);
extern int fclose(void *);
extern int ungetc(int, void *);
extern int fgetc(void *);
extern int feof(void *);
extern int ferror(void *);
extern int setvbuf(void *, char *, int, size_t);
extern long getline(char **, size_t *, void *);
extern long getdelim(char **, size_t *, int, void *);
extern int *__errno_location(void);
struct Source { const char *bytes; size_t length, position, limit; int error; };
static ssize_t read_source(void *cookie, char *out, size_t count) {
    struct Source *s = cookie;
    size_t remaining = s->length - s->position;
    if (!remaining && s->error) { *__errno_location() = s->error; return -1; }
    if (count > remaining) count = remaining;
    if (count > s->limit) count = s->limit;
    memcpy(out, s->bytes + s->position, count);
    s->position += count;
    return count;
}
static void *open_source(struct Source *s) {
    return fopencookie(s, "r", (struct Functions){read_source, 0, 0, 0});
}
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
int probe_lines(void) {
    const char text[] = "first\n\0second\nlast";
    for (unsigned limit = 1; limit < 100; limit = limit * 3 + 1) {
        struct Source s = {text, sizeof(text)-1, 0, limit, 0};
        void *f = open_source(&s); CHECK(f);
        char *line = 0; size_t cap = (size_t)-1;
        CHECK(getline(&line, &cap, f) == 6 && !memcmp(line, "first\n", 7));
        CHECK(ungetc('X', f) == 'X');
        CHECK(getdelim(&line, &cap, 0, f) == 2 && line[0] == 'X' && line[1] == 0 && line[2] == 0);
        CHECK(getline(&line, &cap, f) == 7 && !memcmp(line, "second\n", 8));
        CHECK(getline(&line, &cap, f) == 4 && !memcmp(line, "last", 5));
        *__errno_location() = 123;
        CHECK(getline(&line, &cap, f) == -1 && feof(f) && !ferror(f) && *__errno_location() == 123);
        free(line); CHECK(fclose(f) == 0);
    }
    char *data = malloc(20003); CHECK(data);
    memset(data, 'A', 20003); data[20000] = '\n'; data[20001] = 'Z';
    struct Source s = {data, 20002, 0, 8192, 0};
    void *f = open_source(&s); CHECK(f);
    char *line = malloc(2); size_t cap = 2;
    CHECK(getline(&line, &cap, f) == 20001 && line[20001] == 0 && !memcmp(line, data, 20001));
    CHECK(fgetc(f) == 'Z');
    free(line); CHECK(fclose(f) == 0); free(data);
    s = (struct Source){"abc", 3, 0, 4, 11};
    f = open_source(&s); CHECK(f);
    CHECK(setvbuf(f, 0, 2, 0) == 0);
    line = 0; cap = 0;
    CHECK(getline(&line, &cap, f) == 3 && ferror(f) && !feof(f));
    CHECK(getline(&line, &cap, f) == -1 && *__errno_location() == 11);
    free(line); CHECK(fclose(f) == 0);
    return 0;
}
/* Caller times only reads, using exactly the same C fixture for both builds. */
long benchmark_lines(unsigned bytes) {
    if (!bytes || bytes % 128) return -1;
    char *data = malloc(bytes);
    if (!data) return -2;
    memset(data, 'x', bytes);
    for (size_t i = 127; i < bytes; i += 128) data[i] = '\n';
    struct Source s = {data, bytes, 0, 8192, 0};
    void *f = open_source(&s);
    if (!f) { free(data); return -3; }
    char *line = 0; size_t capacity = 0; long total = 0, n;
    while ((n = getline(&line, &capacity, f)) >= 0) total += n;
    free(line); fclose(f); free(data);
    return total;
}
long benchmark_file_lines(const char *path) {
    void *f = fopen(path, "r");
    if (!f) return -1;
    char *line = 0; size_t capacity = 0; long total = 0, n;
    while ((n = getline(&line, &capacity, f)) >= 0) total += n;
    int error = ferror(f);
    free(line); fclose(f);
    return error ? -2 : total;
}
