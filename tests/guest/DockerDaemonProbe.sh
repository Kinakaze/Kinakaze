# Run entirely inside one managed guest process domain.
set -eu
case "$1" in api|lifecycle|network|network-ipv4) mode=$1;; *) exit 2;; esac
d=$(/bin/busybox mktemp -d /tmp/kinakaze-docker.XXXXXX)
daemon=
printf 'DOCKER_PROBE_DIR=%s\n' "$d"
docker() { /usr/bin/docker --host "unix://$d/docker.sock" "$@"; }
cleanup() {
    status=$?
    trap - EXIT INT TERM
    if [ "$status" -ne 0 ]; then
        printf 'DOCKER_FAILURE_MOUNTINFO\n' >&2
        /bin/busybox cat /proc/self/mountinfo >&2 || true
        if [ "$mode" = network ] || [ "$mode" = network-ipv4 ]; then
            docker inspect --format '{{json .State}}' kinakaze-server >&2 || true
            docker logs kinakaze-server >&2 || true
        fi
    fi
    if [ -n "$daemon" ]; then
        kill -TERM "$daemon" 2>/dev/null || true
        wait "$daemon" || true
    fi
    /bin/busybox cat "$d/daemon.log"
    /bin/busybox rm -rf "$d"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
printf '{}\n' > "$d/daemon.json"
/usr/bin/dockerd --config-file "$d/daemon.json" --host "unix://$d/docker.sock" --data-root "$d/data" --exec-root "$d/run" --pidfile "$d/daemon.pid" > "$d/daemon.log" 2>&1 &
daemon=$!
attempt=0
while [ "$attempt" -lt 300 ]; do
    if ! kill -0 "$daemon" 2>/dev/null; then
        wait "$daemon"
        exit 1
    fi
    if docker info --format '{{.ServerVersion}}' > "$d/info" 2> "$d/client.log"; then
        test -s "$d/info"
        /bin/busybox cat "$d/info"
        printf 'DOCKER_DAEMON_API_OK\n'
        break
    fi
    attempt=$((attempt + 1))
    /bin/busybox sleep 0.1
done
if [ "$attempt" -eq 300 ]; then
    /bin/busybox cat "$d/client.log" >&2
    exit 1
fi
if [ "$mode" = api ]; then exit 0; fi

# BusyBox's libc/resolv dependencies are supplied by the registered V2 providers.
# The image contains real ELF code; there is no registry or network download.
/bin/busybox mkdir -p "$d/rootfs/bin" "$d/rootfs/etc" "$d/rootfs/proc" "$d/rootfs/dev" "$d/rootfs/tmp" "$d/volume"
/bin/busybox cp /bin/busybox "$d/rootfs/bin/busybox"
/bin/busybox chmod 755 "$d/rootfs/bin/busybox"
/bin/busybox ln -s busybox "$d/rootfs/bin/sh"
printf 'root:x:0:0:root:/:/bin/sh\n' > "$d/rootfs/etc/passwd"
printf 'root:x:0:\n' > "$d/rootfs/etc/group"
/bin/busybox tar -C "$d/rootfs" -cf "$d/rootfs.tar" .
docker import "$d/rootfs.tar" kinakaze-probe:local > "$d/image"
test -s "$d/image"
printf 'DOCKER_IMAGE_IMPORT_OK\n'
if [ "$mode" = network ] || [ "$mode" = network-ipv4 ]; then
    printf 'container network payload\n' > "$d/volume/fixture"
    docker network create kinakaze-probe-net > "$d/network"
    listen=8080
    if [ "$mode" = network-ipv4 ]; then listen=0.0.0.0:8080; fi
    docker run -d --name kinakaze-server --network kinakaze-probe-net \
        --mount "type=bind,source=$d/volume,target=/workspace,readonly" \
        kinakaze-probe:local /bin/busybox httpd -f -p "$listen" -h /workspace > "$d/server"
    docker exec kinakaze-server /bin/busybox sh -ec '
        attempt=0
        until /bin/busybox wget -T 1 -q -O /tmp/ready http://127.0.0.1:8080/fixture; do
            attempt=$((attempt + 1))
            if [ "$attempt" -ge 10 ]; then
                exit 1
            fi
            /bin/busybox sleep 0.1
        done
        test "$(/bin/busybox cat /tmp/ready)" = "container network payload"
    '
    printf 'DOCKER_NETWORK_LOOPBACK_OK\n'
    address=$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' kinakaze-server)
    test -n "$address"
    docker run --rm --network kinakaze-probe-net kinakaze-probe:local \
        /bin/busybox wget -T 10 -q -O - "http://$address:8080/fixture" > "$d/response"
    test "$(/bin/busybox cat "$d/response")" = 'container network payload'
    printf 'DOCKER_NETWORK_TCP_OK\n'
    docker run --rm --network kinakaze-probe-net kinakaze-probe:local \
        /bin/busybox wget -T 10 -q -O - http://kinakaze-server:8080/fixture > "$d/dns-response"
    test "$(/bin/busybox cat "$d/dns-response")" = 'container network payload'
    printf 'DOCKER_NETWORK_DNS_OK\n'
    docker rm -f kinakaze-server > "$d/removed"
    docker network rm kinakaze-probe-net > "$d/network-removed"
    docker image rm kinakaze-probe:local > "$d/image-removed"
    test -z "$(docker ps -aq)"
    printf 'DOCKER_CONTAINER_NETWORK_OK\n'
    exit 0
fi
docker run -d --name kinakaze-probe --mount "type=bind,source=$d/volume,target=/workspace" kinakaze-probe:local /bin/busybox sh -ec '
    test "$$" -eq 1
    test -r /proc/self/status
    printf "container volume\n" > /workspace/created
    while [ ! -e /workspace/finish ]; do /bin/busybox sleep 0.1; done
    exit 23
' > "$d/container"
test -s "$d/container"
test "$(docker inspect --format '{{.State.Running}}' kinakaze-probe)" = true
test -n "$(docker inspect --format '{{.NetworkSettings.Networks.bridge.IPAddress}}' kinakaze-probe)"
attempt=0
while [ ! -s "$d/volume/created" ] && [ "$attempt" -lt 100 ]; do
    attempt=$((attempt + 1))
    /bin/busybox sleep 0.1
done
test "$(/bin/busybox cat "$d/volume/created")" = 'container volume'
printf 'DOCKER_CONTAINER_STARTED_OK\n'
docker exec kinakaze-probe /bin/busybox sh -ec '
    test "$$" -gt 1
    test "$(/bin/busybox cat /workspace/created)" = "container volume"
    printf "executed\n" > /workspace/executed
    printf "DOCKER_EXEC_PAYLOAD_OK\n"
'
test "$(/bin/busybox cat "$d/volume/executed")" = executed
printf 'DOCKER_EXEC_OK\n'
# Let exec finish before asking PID 1 to exit. Otherwise namespace teardown can
# legitimately kill the exec command while its exit status is still in flight.
printf 'finished\n' > "$d/volume/finish"
test "$(docker wait kinakaze-probe)" = 23
test "$(/bin/busybox cat "$d/volume/created")" = 'container volume'
test "$(docker inspect --format '{{.State.ExitCode}}' kinakaze-probe)" = 23
printf 'DOCKER_CONTAINER_EXIT_OK\n'
docker start kinakaze-probe > "$d/restarted"
test "$(docker wait kinakaze-probe)" = 23
printf 'DOCKER_CONTAINER_RESTART_OK\n'
docker rm kinakaze-probe > "$d/removed"
docker image rm kinakaze-probe:local > "$d/image-removed"
test -z "$(docker ps -aq)"
printf 'DOCKER_CONTAINER_LIFECYCLE_OK\n'
