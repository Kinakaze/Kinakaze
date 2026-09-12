set -eu
work=$(/bin/busybox mktemp -d /tmp/kinakaze-gcov.XXXXXX)
trap 'cd /; /bin/busybox rm -rf "$work"' EXIT
cd "$work"
/bin/busybox cat > branch.c <<'C'
int choose(int n) {
    if (n > 3) return n * 2;
    return n + 1;
}
int main(void) {
    int sum = 0;
    for (int n = 0; n < 8; n++) sum += choose(n);
    return sum != 54;
}
C
/usr/bin/gcc -O0 --coverage branch.c -o branch
./branch
test -s branch.gcda
/usr/bin/gcov -b -c branch.c > coverage.txt
/bin/busybox cat coverage.txt
/bin/busybox grep 'Lines executed:100.00%' coverage.txt
/bin/busybox grep 'Branches executed:100.00%' coverage.txt
/bin/busybox grep 'Taken at least once:100.00%' coverage.txt
test -s branch.c.gcov
/usr/bin/gcov-dump -l branch.gcda | /bin/busybox grep 'COUNTERS arcs'
printf 'GCOV_EXECUTION_BRANCH_COUNTS_OK\n'
