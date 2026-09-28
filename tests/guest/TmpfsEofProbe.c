#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(test) do { if (!(test)) { fprintf(stderr, "line %d: %s errno=%d\n", __LINE__, #test, errno); exit(1); } } while (0)
static sigjmp_buf jump;
static volatile sig_atomic_t caught, code;
static void *fault_address;
static void fault(int signal, siginfo_t *info, void *context) {
    (void)context;
    caught = signal;
    code = info->si_code;
    fault_address = info->si_addr;
    siglongjmp(jump, 1);
}
static void expect_fault(volatile unsigned char *address, int writing, int signal) {
    caught = 0;
    if (sigsetjmp(jump, 1) == 0) {
        if (writing) *address = 73;
        else { volatile unsigned char value = *address; (void)value; }
    }
    CHECK(caught == signal);
    CHECK(fault_address == (void *)address);
    if (signal == SIGBUS) CHECK(code == BUS_ADRERR);
}
static void waited(pid_t child) {
    int status;
    CHECK(waitpid(child, &status, 0) == child);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
int main(int argc, char **argv) {
    if (argc == 4 && strcmp(argv[1], "truncate") == 0) {
        int fd = open(argv[2], O_RDWR);
        CHECK(fd >= 0);
        CHECK(ftruncate(fd, strtoll(argv[3], NULL, 10)) == 0);
        CHECK(close(fd) == 0);
        return 0;
    }
    const size_t page = sysconf(_SC_PAGESIZE);
    struct sigaction action = {0};
    action.sa_sigaction = fault;
    action.sa_flags = SA_SIGINFO;
    sigemptyset(&action.sa_mask);
    CHECK(sigaction(SIGBUS, &action, NULL) == 0);
    CHECK(sigaction(SIGSEGV, &action, NULL) == 0);
    CHECK(mkdir("tmpfs-eof", 0700) == 0);
    CHECK(mount("tmpfs", "tmpfs-eof", "tmpfs", 0, "size=8m") == 0);
    int fd = open("tmpfs-eof/file", O_CREAT | O_RDWR, 0600);
    CHECK(fd >= 0);
    unsigned char value = 0;
    unsigned char *shared = mmap(NULL, page * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    CHECK(shared != MAP_FAILED);
    expect_fault(shared, 0, SIGBUS);
    expect_fault(shared + page * 3, 1, SIGBUS);
    pid_t child = fork();
    CHECK(child >= 0);
    if (!child) { expect_fault(shared + page, 0, SIGBUS); _exit(0); }
    waited(child);
    child = fork();
    CHECK(child >= 0);
    if (!child) {
        CHECK(signal(SIGBUS, SIG_DFL) != SIG_ERR);
        volatile unsigned char byte = shared[0]; (void)byte;
        _exit(121);
    }
    int signal_status;
    CHECK(waitpid(child, &signal_status, 0) == child);
    CHECK(WIFSIGNALED(signal_status) && WTERMSIG(signal_status) == SIGBUS);
    CHECK(ftruncate(fd, 17) == 0);
    CHECK(shared[16] == 0 && shared[page - 1] == 0);
    shared[16] = 19;
    shared[page - 1] = 25;
    struct stat st;
    CHECK(fstat(fd, &st) == 0 && st.st_size == 17);
    expect_fault(shared + page, 0, SIGBUS);
    // A sparse VMA can exceed both EOF and the mount's physical page quota.
    const size_t far = 16 * 1024 * 1024;
    unsigned char *sparse = mmap(NULL, far * 2, PROT_READ | PROT_WRITE, MAP_SHARED, fd, far);
    CHECK(sparse != MAP_FAILED);
    CHECK(fstat(fd, &st) == 0 && st.st_blocks <= (long)(page / 512));
    expect_fault(sparse, 0, SIGBUS);
    CHECK(ftruncate(fd, far + page) == 0);
    CHECK(sparse[0] == 0);
    memset(sparse, 51, 16);
    CHECK(pread(fd, &value, 1, far) == 1 && value == 51);
    CHECK(munmap(sparse, far * 2) == 0);
    CHECK(ftruncate(fd, page * 3) == 0);
    CHECK(shared[page] == 0 && shared[page * 2] == 0);
    shared[0] = 11; shared[page] = 22; shared[page * 2] = 33;
    CHECK(pread(fd, &value, 1, page * 2) == 1 && value == 33);
    unsigned char *private = mmap(NULL, page * 4, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    CHECK(private != MAP_FAILED);
    private[0] = 91; private[page * 2] = 93;
    child = fork();
    CHECK(child >= 0);
    if (!child) {
        char size[32]; snprintf(size, sizeof(size), "%zu", page + 31);
        execl(argv[0], argv[0], "truncate", "tmpfs-eof/file", size, NULL);
        _exit(120);
    }
    waited(child);
    CHECK(shared[0] == 11 && shared[page] == 22);
    CHECK(private[0] == 91);
    CHECK(shared[page + 31] == 0);
    expect_fault(shared + page * 2, 0, SIGBUS);
    expect_fault(private + page * 2, 1, SIGBUS);
    CHECK(ftruncate(fd, page * 3) == 0);
    CHECK(shared[page * 2] == 0 && private[page * 2] == 0);
    CHECK(private[0] == 91);
    CHECK(mprotect(shared + page * 3, page, PROT_NONE) == 0);
    expect_fault(shared + page * 3, 0, SIGSEGV);
    CHECK(mprotect(shared + page * 3, page, PROT_READ | PROT_WRITE) == 0);
    expect_fault(shared + page * 3, 0, SIGBUS);
    CHECK(mprotect(shared, page, PROT_READ) == 0);
    expect_fault(shared, 1, SIGSEGV);
    CHECK(mprotect(shared, page, PROT_READ | PROT_WRITE) == 0);
    CHECK(private[0] == 91 && shared[0] == 11);
    puts("TMPFS_EOF_REMOTE_COW_OK"); fflush(stdout);

    int source = open("tmpfs-eof/source", O_CREAT | O_RDWR, 0600);
    int destination = open("tmpfs-eof/destination", O_CREAT | O_RDWR, 0600);
    CHECK(source >= 0 && destination >= 0 && ftruncate(source, page) == 0);
    unsigned char *buffer = mmap(NULL, page * 2, PROT_READ | PROT_WRITE, MAP_SHARED, source, 0);
    CHECK(buffer != MAP_FAILED);
    CHECK(write(destination, buffer, 1) == 1);
    buffer[0] = 77;
    CHECK(pread(destination, &value, 1, 0) == 1 && value == 0);
    CHECK(pread(destination, buffer + 1, 1, 0) == 1 && buffer[1] == 0);
    errno = 0;
    CHECK(write(destination, buffer + page, 1) == -1 && errno == EFAULT);
    errno = 0;
    CHECK(pread(destination, buffer + page, 1, 0) == -1 && errno == EFAULT);
    expect_fault(buffer + page, 0, SIGBUS);
    CHECK(munmap(buffer, page * 2) == 0);
    CHECK(ftruncate(source, page * 4) == 0);
    buffer = mmap(NULL, page * 2, PROT_READ | PROT_WRITE, MAP_SHARED, source, page * 2);
    CHECK(buffer != MAP_FAILED);
    int native = open("native-buffer", O_CREAT | O_RDWR, 0600);
    CHECK(native >= 0 && write(native, "N", 1) == 1);
    CHECK(pread(native, buffer, 1, 0) == 1 && buffer[0] == 'N');
    CHECK(pwrite(native, buffer + page, 1, 0) == 1);
    CHECK(pread(native, &value, 1, 0) == 1 && value == 0);
    CHECK(ftruncate(source, page * 2) == 0);
    errno = 0;
    CHECK(pread(native, buffer, 1, 0) == -1 && errno == EFAULT);
    CHECK(pread(native, buffer, 1, page) == 0);
    CHECK(close(native) == 0 && munmap(buffer, page * 2) == 0);
    CHECK(ftruncate(source, page * 6) == 0);
    buffer = mmap(NULL, page * 2, PROT_READ | PROT_WRITE, MAP_SHARED, source, page * 4);
    CHECK(buffer != MAP_FAILED);
    int pipes[2];
    CHECK(pipe(pipes) == 0);
    CHECK(write(pipes[1], buffer + page, 1) == 1);
    CHECK(read(pipes[0], buffer, 1) == 1 && buffer[0] == 0);
    CHECK(close(pipes[0]) == 0 && close(pipes[1]) == 0);
    CHECK(munmap(buffer, page * 2) == 0);
    CHECK(close(source) == 0 && close(destination) == 0);
    puts("TMPFS_EOF_SYSCALL_BUFFERS_OK"); fflush(stdout);

    // Forked views must enroll before guest execution, including reservations
    // beyond EOF and dirty private pages inherited from the parent.
    child = fork();
    CHECK(child >= 0);
    if (!child) {
        CHECK(private[0] == 91);
        CHECK(ftruncate(fd, 0) == 0);
        expect_fault(shared, 0, SIGBUS);
        expect_fault(private, 0, SIGBUS);
        _exit(0);
    }
    waited(child);
    expect_fault(shared, 0, SIGBUS);
    expect_fault(private, 0, SIGBUS);
    CHECK(ftruncate(fd, page * 3) == 0);
    CHECK(shared[0] == 0 && private[0] == 0);

    CHECK(munmap(shared + page * 3, page) == 0);
    expect_fault(shared + page * 3, 0, SIGSEGV);
    unsigned char *anonymous = mmap(shared + page, page, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    CHECK(anonymous == shared + page);
    anonymous[0] = 47;
    CHECK(ftruncate(fd, 0) == 0);
    CHECK(anonymous[0] == 47);
    expect_fault(shared, 0, SIGBUS);
    expect_fault(shared + page * 2, 0, SIGBUS);
    CHECK(unlink("tmpfs-eof/file") == 0);
    CHECK(ftruncate(fd, page * 3) == 0);
    CHECK(shared[0] == 0);
    CHECK(close(fd) == 0);
    child = fork();
    CHECK(child >= 0);
    if (!child) { CHECK(shared[0] == 0); shared[0] = 80; _exit(0); }
    waited(child);
    CHECK(shared[0] == 80);
    int other = open("tmpfs-eof/file", O_CREAT | O_RDWR, 0600);
    CHECK(other >= 0);
    CHECK(write(other, "new", 3) == 3);
    shared[0] = 79;
    CHECK(pread(other, &value, 1, 0) == 1 && value == 'n');
    CHECK(close(other) == 0);
    CHECK(munmap(shared, page * 4) == 0);
    CHECK(munmap(private, page * 4) == 0);
    CHECK(umount2("tmpfs-eof", MNT_DETACH) == 0);
    CHECK(rmdir("tmpfs-eof") == 0);
    puts("TMPFS_EOF_LIFETIMES_OK");
    CHECK(mkdir("tmpfs-quota", 0700) == 0);
    char quota[40]; snprintf(quota, sizeof(quota), "size=%zu", page);
    CHECK(mount("tmpfs", "tmpfs-quota", "tmpfs", 0, quota) == 0);
    fd = open("tmpfs-quota/file", O_CREAT | O_RDWR, 0600);
    CHECK(fd >= 0 && ftruncate(fd, page * 2) == 0);
    shared = mmap(NULL, page * 2, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    CHECK(shared != MAP_FAILED);
    shared[0] = 5;
    expect_fault(shared + page, 1, SIGBUS);
    CHECK(ftruncate(fd, 0) == 0 && ftruncate(fd, page * 2) == 0);
    CHECK(shared[page] == 0);
    expect_fault(shared, 0, SIGBUS);
    CHECK(munmap(shared, page * 2) == 0 && close(fd) == 0);
    CHECK(umount2("tmpfs-quota", MNT_DETACH) == 0);
    CHECK(rmdir("tmpfs-quota") == 0);
    puts("TMPFS_EOF_QUOTA_OK");
    return 0;
}
