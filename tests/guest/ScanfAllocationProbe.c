#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "check failed at %d\n", __LINE__); return 1; } } while (0)

int main(void) {
    char *word = NULL, *command = NULL, *characters = NULL;
    int value = 0, consumed = 0;
    CHECK(sscanf(" hello 42", "%ms %d%n", &word, &value, &consumed) == 2);
    CHECK(word && !strcmp(word, "hello") && value == 42 && consumed == 9);
    free(word);
    CHECK(sscanf("worker name) R 42", "%m[^)]) %mc %d", &command, &characters, &value) == 3);
    CHECK(!strcmp(command, "worker name") && characters[0] == 'R' && value == 42);
    free(command); free(characters);
    word = NULL;
    CHECK(sscanf("abcdef", "%3ms%n", &word, &consumed) == 1);
    CHECK(!strcmp(word, "abc") && consumed == 3); free(word);
    characters = NULL;
    CHECK(sscanf("abcd", "%3mc%n", &characters, &consumed) == 1);
    CHECK(!memcmp(characters, "abc", 3) && consumed == 3); free(characters);
    word = NULL;
    CHECK(sscanf("abc", "%m[0-9]", &word) == 0 && word == NULL);
    CHECK(sscanf("", "%ms", &word) == EOF && word == NULL);
    CHECK(sscanf("skip 42", "%*ms %d", &value) == 1 && value == 42);
    FILE *file = tmpfile();
    CHECK(file != NULL);
    CHECK(fputs("file-word 17", file) >= 0); rewind(file);
    CHECK(fscanf(file, "%ms %d", &word, &value) == 2);
    CHECK(!strcmp(word, "file-word") && value == 17); free(word); fclose(file);
    for (int i = 0; i < 1000; ++i) {
        word = NULL;
        CHECK(sscanf("repeat", "%ms", &word) == 1 && !strcmp(word, "repeat"));
        free(word);
    }
    puts("SCANF_ALLOCATION_OK");
    return 0;
}
