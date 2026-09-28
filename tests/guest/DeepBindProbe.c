#define _GNU_SOURCE
#include <assert.h>
#include <dlfcn.h>
#include <stdio.h>

#if defined(DEPENDENCY)
int dependency_value(void) { return 20; }
#elif defined(PLUGIN)
extern int dependency_value(void);
int own_value(void) { return 2; }
int combined_value(void) { return dependency_value() + own_value(); }
#else
int dependency_value(void) { return 100; }
int own_value(void) { return 1; }
typedef int (*function)(void);
int main(void) {
    void *plain = dlopen("./plain.so", RTLD_NOW | RTLD_LOCAL);
    if (!plain) { puts(dlerror()); return 1; }
    function ordinary = (function)dlsym(plain, "combined_value");
    assert(ordinary && ordinary() == 101);
    void *deep = dlopen("./deep.so", RTLD_NOW | RTLD_LOCAL | RTLD_DEEPBIND);
    if (!deep) { puts(dlerror()); return 2; }
    function isolated = (function)dlsym(deep, "combined_value");
    assert(isolated && isolated() == 22);
    assert(((function)dlsym(RTLD_DEFAULT, "dependency_value"))() == 100);
    assert(((function)dlsym(RTLD_DEFAULT, "own_value"))() == 1);
    assert(ordinary() == 101);
    assert(dlclose(deep) == 0 && dlclose(plain) == 0);
    puts("DEEPBIND_SCOPE_OK");
    return 0;
}
#endif
