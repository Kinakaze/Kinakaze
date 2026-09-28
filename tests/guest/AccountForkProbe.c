#include <assert.h>
#include <grp.h>
#include <pwd.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static void child_ok(pid_t child) {
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
static void inspect(struct passwd *user, struct group *group) {
    assert(user->pw_uid == 0 && group->gr_gid == 0);
    assert(strcmp(user->pw_name, "root") == 0 && strcmp(group->gr_name, "root") == 0);
    assert(user->pw_dir[0] == '/' && user->pw_shell[0] == '/');
    assert(group->gr_mem && group->gr_passwd);
    for (char **member = group->gr_mem; *member; ++member) assert(strlen(*member) < 1024);
}
int main(void) {
    struct passwd *user = getpwnam("root");
    struct group *group = getgrnam("root");
    assert(user && group);
    inspect(user, group);
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
        inspect(user, group);
        assert(getpwnam("nobody") && getgrnam("nogroup"));
        _exit(0);
    }
    child_ok(child);
    inspect(user, group);

    // Fork copies the enumeration positions independently of later reads.
    char next_user[256], next_group[256];
    setpwent();
    assert(getpwent());
    user = getpwent();
    assert(user && strlen(user->pw_name) < sizeof next_user);
    strcpy(next_user, user->pw_name);
    setgrent();
    assert(getgrent());
    group = getgrent();
    assert(group && strlen(group->gr_name) < sizeof next_group);
    strcpy(next_group, group->gr_name);
    setpwent(); assert(getpwent());
    setgrent(); assert(getgrent());
    child = fork();
    assert(child >= 0);
    if (!child) {
        user = getpwent(); group = getgrent();
        assert(user && group);
        assert(strcmp(user->pw_name, next_user) == 0 && strcmp(group->gr_name, next_group) == 0);
        endpwent(); endgrent();
        _exit(0);
    }
    child_ok(child);
    user = getpwent(); group = getgrent();
    assert(user && group);
    assert(strcmp(user->pw_name, next_user) == 0 && strcmp(group->gr_name, next_group) == 0);
    endpwent(); endgrent();
    puts("ACCOUNT_FORK_OK");
}
