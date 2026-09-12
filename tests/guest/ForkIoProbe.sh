set -eu
d=$(/bin/busybox mktemp -d /tmp/kinakaze-fork-io.XXXXXX)
trap '/bin/busybox rm -rf "$d"' EXIT

exec 3> "$d/shared"
exec 4>&3
printf A >&3
/bin/busybox printf B >&4
printf C >&4
(
    exec 3>&-
    /bin/busybox printf D >&4
)
printf E >&3
exec 3>&-
exec 4>&-
test "$(/bin/busybox cat "$d/shared")" = ABCDE

# stdout/stderr aliases remain one description across multiple exec workers.
(
    /bin/busybox sh -ec 'printf "out\n"; printf "err\n" >&2'
    printf 'parent\n' >&2
) > "$d/log" 2>&1
printf 'out\nerr\nparent\n' > "$d/expected"
/bin/busybox cmp "$d/log" "$d/expected"
printf 'FORK_SHARED_IO_OK\n'
