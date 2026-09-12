set -eu
work=$(/bin/busybox mktemp -d /tmp/kinakaze-binutils.XXXXXX)
trap 'cd /; /bin/busybox rm -rf "$work"' EXIT
cd "$work"
/bin/busybox cat > value.c <<'C'
int twice(int n) { return n * 2; }
C
/bin/busybox cat > main.c <<'C'
#include <stdio.h>
extern int twice(int);
int main(void) { puts("BINUTILS_LINKED_PROGRAM_OK"); return twice(21) != 42; }
C
/usr/bin/gcc -g -O0 -c value.c -o value.o
/usr/bin/ar qc libvalue.a value.o
/usr/bin/ranlib libvalue.a
test "$(/usr/bin/ar t libvalue.a)" = value.o
/usr/bin/nm libvalue.a | /bin/busybox grep ' T twice'
/usr/bin/gcc -g -O0 -no-pie main.c libvalue.a -o app
./app
/usr/bin/readelf -h app | /bin/busybox grep 'ELF64'
/usr/bin/objdump -d app | /bin/busybox grep '<twice>:'
/usr/bin/objcopy --only-keep-debug app app.debug
/usr/bin/strip --strip-debug app
/usr/bin/objcopy --add-gnu-debuglink=app.debug app
/usr/bin/readelf -S app | /bin/busybox grep '.gnu_debuglink'
address=$(/usr/bin/nm -n app.debug | /bin/busybox awk '$3 == "twice" { print $1 }')
test -n "$address"
/usr/bin/addr2line -e app.debug "$address" | /bin/busybox grep 'value.c:1'
/usr/bin/strings app | /bin/busybox grep '^BINUTILS_LINKED_PROGRAM_OK$'
/usr/bin/size app | /bin/busybox grep 'app'
test "$(/usr/bin/c++filt _Z5helloi)" = 'hello(int)'
/usr/bin/elfedit --output-osabi Linux app
./app
printf 'BINUTILS_ARCHIVE_DEBUG_STRIP_SYMBOLS_OK\n'
