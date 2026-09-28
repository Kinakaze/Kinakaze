#include <assert.h>
#include <dlfcn.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static void load(const char *path) {
    for (int i = 0; i != 2; ++i) {
        void *module = dlopen(path, RTLD_NOW | RTLD_LOCAL);
        if (!module) fprintf(stderr, "dlopen: %s\n", dlerror());
        assert(module);
        int (*increment)(void) = dlsym(module, "increment");
        assert(increment);
        int value = increment();
        // dlclose releases this handle; an implementation may retain the ELF
        // mapping. A reopen then keeps TLS instead of reinitializing it.
        assert(value == 41 || (i == 1 && value == 43));
        assert(increment() == value + 1);
        assert(dlclose(module) == 0);
    }
}
static void joined(pid_t child) {
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
int main(int argc, char **argv) {
    assert(argc == 2);
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
        pid_t grandchild = fork();
        assert(grandchild >= 0);
        if (!grandchild) { load(argv[1]); _exit(0); }
        joined(grandchild);
        load(argv[1]);
        _exit(0);
    }
    joined(child);
    load(argv[1]);
    puts("FORK_DLOPEN_OK");
}
