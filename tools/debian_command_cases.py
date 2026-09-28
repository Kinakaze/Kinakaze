"""Explicit, bounded functional scenarios for Debian's standard command set."""

import bz2
import gzip
import hashlib
import io
import lzma
from pathlib import Path, PurePosixPath
import shlex
import tarfile
import zipfile


def cases():
    result = {}

    def add(names, script, purpose, timeout=12):
        for name in names.split():
            if name in result:
                raise ValueError("duplicate command case: " + name)
            result[name] = dict(script=script, purpose=purpose, timeout=timeout)

    def output(names, args, pattern, purpose, stdin=None, codes=None):
        invocation = '"$1" ' + args
        if stdin is not None:
            invocation = "printf %s " + shlex.quote(stdin) + " | " + invocation
        if codes:
            invocation = (
                "code=0; "
                + invocation
                + ' > out 2> err || code=$?; case "$code" in '
                + "|".join(map(str, codes))
                + ") ;; *) cat err; exit 1;; esac"
            )
        else:
            invocation += " > out"
        check = (
            "" if pattern == ".*" else "\ngrep -E -- " + shlex.quote(pattern) + " out"
        )
        add(names, invocation + "\ncat out" + check, purpose)

    def exact(names, args, expected, purpose, stdin=None):
        invocation = '"$1" ' + args
        if stdin is not None:
            invocation = "printf %s " + shlex.quote(stdin) + " | " + invocation
        add(
            names,
            invocation
            + " > out\nprintf %s "
            + shlex.quote(expected)
            + " > expected\ncmp out expected",
            purpose,
        )

    exact(
        "bash dash sh ash rbash",
        '-c \'a=hello; printf "%s:%s" "$a" "$((6*7))"\'',
        "hello:42",
        "shell variables and arithmetic",
    )
    exact(
        "busybox", "awk 'BEGIN { print 6*7 }'", "42\n", "BusyBox awk applet execution"
    )
    add("true", '"$1"', "successful predicate exit status")
    add("false", 'code=0; "$1" || code=$?; test "$code" = 1', "false predicate exits 1")
    add(
        "test",
        '"$1" -f text.txt; code=0; "$1" -f missing || code=$?; test "$code" = 1',
        "file predicates and negative result",
    )
    add(
        "[",
        '"$1" -f text.txt ]; code=0; "$1" -f missing ] || code=$?; test "$code" = 1',
        "bracket predicates and negative result",
    )
    exact("echo", "audit text", "audit text\n", "argument formatting")
    exact("printf", "'%s:%04d' hello 42", "hello:0042", "string and integer formatting")
    output("cat", "text.txt", "^alpha beta$", "read file content")
    exact("tac", "lines.txt", "three\ntwo\none\n", "reverse line order")
    output("ls dir vdir", "-l text.txt", "text.txt", "directory entry metadata")
    output("pwd", "", "/tmp/command-audit/", "current working directory")
    exact(
        "basename",
        "/tmp/example.txt .txt",
        "example\n",
        "path basename and suffix removal",
    )
    exact("dirname", "/tmp/example.txt", "/tmp\n", "path parent extraction")
    output("realpath", "text.txt", "/text.txt$", "canonical path resolution")
    add(
        "readlink",
        'ln -s text.txt alias; test "$("$1" alias)" = text.txt',
        "symbolic link target",
    )
    add(
        "cp",
        '"$1" text.txt copy; cmp text.txt copy; "$1" -p copy copy2; cmp copy copy2',
        "copy data and metadata",
    )
    add(
        "mv",
        'cp text.txt source; "$1" source moved; test ! -e source; cmp text.txt moved',
        "rename and preserve data",
    )
    add(
        "rm unlink",
        'cp text.txt disposable; "$1" disposable; test ! -e disposable',
        "remove isolated file",
    )
    add("mkdir", '"$1" -p a/b; test -d a/b', "recursive directory creation")
    add("rmdir", 'mkdir empty; "$1" empty; test ! -e empty', "remove empty directory")
    add(
        "touch",
        '"$1" -d @1000000000 touched; test "$(stat -c %Y touched)" = 1000000000',
        "create and set timestamp",
    )
    add(
        "ln link",
        '"$1" text.txt linked; cmp text.txt linked; test "$(stat -c %i text.txt)" = "$(stat -c %i linked)"',
        "hard link inode identity",
    )
    add(
        "chmod",
        '"$1" 640 text.txt; test "$(stat -c %a text.txt)" = 640',
        "permission update",
    )
    add(
        "chown",
        '"$1" 1234:2345 text.txt; test "$(stat -c %u:%g text.txt)" = 1234:2345',
        "numeric owner and group update",
    )
    add(
        "chgrp",
        '"$1" 2345 text.txt; test "$(stat -c %g text.txt)" = 2345',
        "group update",
    )
    output("stat", '-c "%s %F" text.txt', "^33 regular file$", "file size and type")
    output("file", "text.txt", "ASCII text", "content-based file identification")
    add(
        "install",
        '"$1" -m 640 text.txt installed; cmp text.txt installed; test "$(stat -c %a installed)" = 640',
        "install file with requested mode",
    )
    add(
        "mktemp",
        'p=$("$1" -p "$PWD" audit.XXXXXX); test -f "$p"; rm "$p"',
        "unique temporary file",
    )
    add(
        "tempfile",
        'p=$("$1" -d "$PWD"); test -f "$p"; rm "$p"',
        "temporary file allocation",
    )
    add("mkfifo", '"$1" channel; test -p channel', "create FIFO inode")
    add("mknod", '"$1" channel p; test -p channel', "mknod FIFO type")
    add(
        "truncate",
        '"$1" -s 4097 sparse; test "$(stat -c %s sparse)" = 4097',
        "set file length",
    )
    add(
        "fallocate",
        '"$1" -l 4096 allocated; test "$(stat -c %s allocated)" = 4096',
        "allocate file range",
    )
    add(
        "dd",
        '"$1" if=text.txt of=copy bs=3 status=none; cmp text.txt copy',
        "block copy",
    )
    output("du", "-b text.txt", "^33[[:space:]]", "apparent file usage")
    output("df", "-P .", "Filesystem", "filesystem capacity query")
    add("sync", '"$1" text.txt', "flush file data")
    add(
        "shred",
        'cp text.txt disposable; "$1" -n 1 -z -u disposable; test ! -e disposable',
        "overwrite and remove an isolated file",
    )
    output("namei", "-l text.txt", "text.txt", "walk path component metadata")
    add(
        "hardlink",
        'cp text.txt duplicate; "$1" -c text.txt duplicate; test "$(stat -c %i text.txt)" = "$(stat -c %i duplicate)"',
        "coalesce files with identical contents",
    )
    add(
        "rename.ul",
        'cp text.txt old.txt; "$1" old new old.txt; test -f new.txt',
        "rename by substitution",
    )
    add("pathchk", '"$1" -p good/path.txt', "portable pathname validation")
    output(
        "find",
        ". -maxdepth 1 -name text.txt -type f",
        "./text.txt",
        "name and type filtering",
    )
    exact(
        "xargs",
        "-n 2 printf '%s:%s\\n'",
        "a:b\nc:d\n",
        "argument batching",
        "a b c d\n",
    )
    exact("sort", "", "a\nb\nc\n", "text sorting", "c\na\nb\n")
    exact(
        "uniq", "-c", "      2 a\n      1 b\n", "count adjacent duplicates", "a\na\nb\n"
    )
    exact("cut", "-d : -f 2", "b\nd\n", "field extraction", "a:b\nc:d\n")
    exact("paste", "-d : left.txt right.txt", "a:1\nb:2\n", "merge file columns")
    exact("join", "join1.txt join2.txt", "a one 1\nb two 2\n", "join on sorted keys")
    exact("comm", "-12 left.txt same.txt", "a\nb\n", "compare sorted files")
    exact("tr", "'a-z' 'A-Z'", "ABC\n", "character translation", "abc\n")
    exact("head", "-n 2 lines.txt", "one\ntwo\n", "first lines")
    exact("tail", "-n 2 lines.txt", "two\nthree\n", "last lines")
    output("wc", "-l lines.txt", "^3 ", "line counting")
    exact("rev", "", "cba\n", "reverse characters", "abc\n")
    output("nl", "-ba lines.txt", "1[[:space:]]+one", "number text lines")
    exact(
        "sed",
        "'s/alpha/omega/g' text.txt",
        "omega beta\nBeta gamma\nomega beta\n",
        "substitution",
    )
    output("grep fgrep rgrep", "alpha text.txt", "^alpha beta$", "matching lines")
    output(
        "egrep",
        "'^(alpha|Beta)' text.txt",
        "^Beta gamma$",
        "extended regular expressions",
    )
    exact(
        "mawk awk",
        "'{n += $1} END {print n}'",
        "6\n",
        "awk record processing",
        "1\n2\n3\n",
    )
    exact("expr", "6 '*' 7", "42\n", "integer expression evaluation")
    exact("seq", "2 2 6", "2\n4\n6\n", "numeric sequence")
    exact("factor", "84", "84: 2 2 3 7\n", "integer factorization")
    exact("numfmt", "--from=iec 2K", "2048\n", "unit conversion")
    exact("expand", "-t 4", "a   b\n", "tab expansion", "a\tb\n")
    exact("unexpand", "-a -t 4", "a\tb\n", "space compression", "a   b\n")
    exact("fold", "-w 3", "abc\ndef\n", "wrap long lines", "abcdef\n")
    exact("fmt", "-w 80", "one two three\n", "paragraph reflow", "one two\nthree\n")
    output("pr", "-t -n lines.txt", "one", "paginated numbered text")
    output("ptx", "-A lines.txt", "three", "permuted text index")
    exact("tsort", "", "a\nb\nc\n", "topological ordering", "a b\nb c\n")
    add(
        "shuf",
        '"$1" -i 1-10 > out; test "$(sort -n out | uniq | wc -l)" = 10',
        "shuffle a complete range",
    )
    exact("tee", "copy", "copied\n", "copy standard input", "copied\n")
    add(
        "split",
        '"$1" -l 1 lines.txt part; cat part* > out; cmp lines.txt out',
        "split and reconstruct lines",
    )
    add(
        "csplit",
        '"$1" lines.txt 2 > sizes; cat xx00 xx01 > out; cmp lines.txt out',
        "split at line boundary",
    )
    add(
        "cmp",
        '"$1" left.txt same.txt; code=0; "$1" left.txt right.txt || code=$?; test "$code" = 1',
        "equal and unequal byte comparison",
    )
    add(
        "diff",
        '"$1" left.txt same.txt; code=0; "$1" -u left.txt right.txt > out || code=$?; test "$code" = 1; grep -q "^-a" out',
        "unified diff and exit status",
    )
    output(
        "diff3",
        "left.txt same.txt right.txt",
        "^====",
        "three-way differences",
        codes=(0, 1),
    )
    output(
        "sdiff", "left.txt right.txt", "a.*1", "side-by-side differences", codes=(1,)
    )
    output("od", "-An -tx1 bytes.bin", "00 01 7f ff", "binary byte dump")
    output("hexdump hd", "-C bytes.bin", "00 01 7f ff", "hexadecimal dump")
    exact("xxd", "-p bytes.bin", "00017fff\n", "binary-to-hex conversion")
    for name, algorithm in [
        ("md5sum", "md5"),
        ("md5sum.textutils", "md5"),
        ("sha1sum", "sha1"),
        ("sha224sum", "sha224"),
        ("sha256sum", "sha256"),
        ("sha384sum", "sha384"),
        ("sha512sum", "sha512"),
        ("b2sum", "blake2b"),
    ]:
        output(
            name,
            "text.txt",
            "^"
            + hashlib.new(
                algorithm, b"alpha beta\nBeta gamma\nalpha beta\n"
            ).hexdigest(),
            "known file digest",
        )
    output("cksum sum", "text.txt", "^[0-9]+[[:space:]]+[0-9]+", "file checksum")
    exact("base64", "", "aGVsbG8=\n", "base64 encoding", "hello")
    exact("base32", "", "NBSWY3DP\n", "base32 encoding", "hello")
    exact("basenc", "--base16", "68656C6C6F\n", "base16 encoding", "hello")
    exact(
        "env",
        "AUDIT_VALUE=works /bin/sh -c 'printf %s \"$AUDIT_VALUE\"'",
        "works",
        "child environment injection",
    )
    add(
        "printenv",
        'export AUDIT_VALUE=works; test "$("$1" AUDIT_VALUE)" = works',
        "environment lookup",
    )
    exact(
        "nice",
        "-n 1 /usr/bin/printf nice-ok",
        "nice-ok",
        "launch with adjusted niceness",
    )
    exact("nohup", "/usr/bin/printf nohup-ok", "nohup-ok", "launch ignoring hangup")
    exact(
        "stdbuf",
        "-o0 /usr/bin/printf buffer-ok",
        "buffer-ok",
        "unbuffered child output",
    )
    add(
        "timeout",
        'code=0; "$1" 0.1 /bin/sleep 10 || code=$?; test "$code" = 124',
        "terminate overlong child",
    )
    add("sleep", '"$1" 0.01', "bounded timed wait")
    exact(
        "yes", "audit | head -n 2", "audit\naudit\n", "repeat output until pipe closes"
    )
    exact("date", "-u -d @0 +%Y-%m-%d", "1970-01-01\n", "parse epoch and format UTC")
    output("uname arch", "", ".+", "system identity query")
    output(
        "hostname dnsdomainname domainname nisdomainname ypdomainname",
        "",
        ".*",
        "read host/domain identity",
    )
    output("hostid", "", "^[0-9a-f]{8}$", "host identifier")
    output("id", "root", "uid=0", "account identity lookup")
    output("groups", "root", "root", "group membership lookup")
    exact("whoami", "", "root\n", "effective user name")
    output("getent", "passwd root", "^root:.*:0:0:", "NSS passwd lookup")
    output("getconf", "PAGESIZE", "^[1-9][0-9]*$", "system configuration value")
    output("locale", "charmap", "UTF-8", "locale encoding query")
    exact(
        "iconv piconv",
        "-f UTF-8 -t ISO-8859-1 ascii.txt",
        "hello\n",
        "character encoding conversion",
    )
    output("localedef", "--list-archive", ".*", "locale archive enumeration")
    exact("gettext", "audit-message", "audit-message", "untranslated message fallback")
    exact("ngettext", "one many 2", "many", "plural-message selection")
    add(
        "envsubst",
        'export AUDIT_WORD=hello; printf \'$AUDIT_WORD world\' | "$1" > out; test "$(cat out)" = "hello world"',
        "environment substitution",
    )
    output(
        "getopt",
        "-o ab: -- -a -b value tail",
        "-a -b 'value' -- 'tail'",
        "option parser quoting",
    )
    output("dircolors", "-b", "LS_COLORS=", "shell color configuration")
    output("which which.debianutils", "sh", "/(usr/)?bin/sh", "PATH command resolution")
    output("whereis", "sh", "^sh:", "binary/manual path discovery")
    add(
        "run-parts",
        "mkdir scripts; printf '#!/bin/sh\\necho script-ok\\n' > scripts/probe; chmod +x scripts/probe; \"$1\" scripts > out; grep -q script-ok out",
        "execute scripts from a directory",
    )
    output("tree", "-L 1 .", "text.txt", "directory tree traversal")
    output("lsb_release", "-is", "^Kinakaze$", "distribution identity")
    add(
        "flock",
        "\"$1\" lock /bin/sh -c 'echo locked > result'; grep -q locked result",
        "advisory file lock and child",
    )
    exact("setsid", "/usr/bin/printf session-ok", "session-ok", "new session execution")
    output("taskset", "-pc $$", "affinity", "read CPU affinity")
    output("chrt", "-p $$", "scheduling", "read scheduling policy")
    output("ionice", "-p $$", ".+", "read IO priority")
    output("prlimit", "--pid $$ --nofile", "NOFILE", "read process resource limit")
    output("nproc", "", "^[1-9][0-9]*$", "available CPU count")
    output("ps", "-p $$ -o pid=,comm=", "[0-9]+.*bash", "process listing")
    output("free", "-b", "Mem:", "memory statistics")
    output("vmstat", "", "free", "virtual memory statistics")
    output("uptime", "", "load average", "uptime and load query")
    output("w", "-h", ".*", "logged-in session query")
    output(
        "who users last lastb lastlog pinky", "", ".*", "login/session accounting query"
    )
    output("pmap", "$$", "[0-9a-f]+", "process memory mappings")
    output("pstree pstree.x11", "-p $$", "bash", "process tree query")
    output("pwdx", "$$", "/tmp/command-audit/", "process working directory query")
    output("prtstat", "$$", "Process:", "process stat decoding")
    output("pgrep", "-x bash", "[0-9]+", "process matching")
    add("kill", '"$1" -0 $$', "signal-zero process existence check")
    add(
        "killall pkill",
        'code=0; "$1" --exact __kinakaze_no_such_process_926f54b__ 2> err || code=$?; test "$code" = 1',
        "negative process-name lookup without sending signals",
    )
    output(
        "lsof",
        "-p $$ -a -d cwd -Fn",
        "^n/tmp/command-audit/",
        "open-file working directory",
    )
    output("lsfd", "-p $$", "(COMMAND|bash)", "file descriptor metadata query")
    output("lslocks", "", ".*", "active file lock query")
    output("lslogins", "-u", "root", "account information table")
    output("lsipc ipcs", "", ".*", "System V IPC inventory")
    output("lsns", "-p $$", "(NS|TYPE)", "process namespaces")
    output("lscpu", "", "CPU", "CPU topology query")
    output("lsmem", "", "(RANGE|Memory)", "memory layout query")
    output("lsirq", "", "(IRQ|CPU)", "interrupt statistics")
    output("top", "-b -n 1", "Tasks:", "single batch process snapshot")
    output("sysctl", "-n kernel.hostname", ".+", "read kernel hostname sysctl")
    output("findmnt", "-n -o TARGET", "/", "mount table query")
    output("mount", "", " on ", "mounted filesystem listing")
    output("mountpoint", "-q /", ".*", "root mountpoint predicate")
    output("lsblk", "", "(NAME|loop|disk)", "block-device listing")
    output("losetup", "-a", ".*", "loop-device query")
    output("swapon", "--show", ".*", "swap inventory")
    output("getcap", "text.txt", ".*", "file capability query")
    output("getpcaps", "$$", "cap_", "process capability query")
    output("capsh", "--print", "Current:", "capability state")
    output("setpriv", "--dump", "(uid|UID)", "privilege state")
    output("chage", "-l root", "Password", "password aging query")
    output("passwd", "-S root", "^root ", "password status query")
    output("faillog", "-u root", ".*", "failed-login accounting query")
    output("faillock", "--user root", ".*", "PAM failure record query")
    output("lsattr", "text.txt", "text.txt", "inode attribute query")
    output("fincore", "text.txt", "(text.txt|FILE)", "page cache residency query")
    output("filefrag", "text.txt", "extent", "file extent query")
    output("ip", "-j addr show", "addr_info", "network interface address dump")
    output("ss", "-an", "(State|Netid)", "socket state dump")
    output("bridge", "link show", ".*", "bridge link query")
    output("tc", "qdisc show", ".*", "queue discipline query")
    output("nstat", "-az", "(Ip|Tcp|Udp)", "network protocol counters")
    output("lnstat ctstat rtstat", "-c 1", ".+", "kernel network statistics")
    output("rdma", "link show", ".*", "RDMA link query")
    output("devlink", "dev show", ".*", "network device management inventory")
    output("dcb", "app show dev lo", ".*", "DCB application query")
    output("genl", "ctrl list", ".*", "generic netlink family query")
    output("nft", "list ruleset", ".*", "netfilter ruleset query")
    output("ping ping4", "-n -c 1 -W 2 127.0.0.1", "1 received", "IPv4 loopback ICMP")
    output("ping6", "-n -c 1 -W 2 ::1", "1 received", "IPv6 loopback ICMP")
    output(
        "curl",
        '--noproxy "*" -fsS --max-time 4 "$AUDIT_HTTP_URL"',
        "^kinakaze-http-ok$",
        "local HTTP response body",
    )
    output(
        "wget",
        '--no-proxy -q -T 4 -O - "$AUDIT_HTTP_URL"',
        "^kinakaze-http-ok$",
        "local HTTP response body",
    )
    add(
        "nc nc.traditional",
        'printf \'GET / HTTP/1.0\\r\\n\\r\\n\' | "$1" -w 3 127.0.0.1 "$AUDIT_HTTP_PORT" > out; grep -q kinakaze-http-ok out',
        "TCP request/response through netcat",
    )
    output(
        "dig mdig",
        '@127.0.0.1 -p "$AUDIT_DNS_PORT" audit.invalid A +time=1 +tries=1',
        "127.0.0.42",
        "query controlled local DNS server",
    )
    output(
        "host",
        '-p "$AUDIT_DNS_PORT" audit.invalid 127.0.0.1',
        "127.0.0.42",
        "query controlled local DNS server",
    )
    output(
        "nslookup",
        '-port="$AUDIT_DNS_PORT" audit.invalid 127.0.0.1',
        "127.0.0.42",
        "query controlled local DNS server",
    )
    output(
        "ssh slogin",
        "-G localhost",
        "^hostname localhost$",
        "SSH client configuration expansion",
    )
    add("scp", '"$1" text.txt copied; cmp text.txt copied', "local-to-local scp copy")
    add(
        "ssh-keygen",
        '"$1" -q -t ed25519 -N "" -f key; "$1" -lf key.pub > out; grep -q ED25519 out',
        "generate and fingerprint Ed25519 key",
    )
    add(
        "openssl",
        '"$1" dgst -sha256 text.txt > out; grep -q '
        + hashlib.sha256(b"alpha beta\nBeta gamma\nalpha beta\n").hexdigest()
        + ' out; "$1" rand -out random 32; test "$(stat -c %s random)" = 32',
        "known SHA256 and cryptographic random bytes",
    )
    output("apt", "list --installed", "kinakaze-base", "APT installed-package listing")
    output("apt-get", "check", "(Reading|Building)", "APT dependency consistency")
    output(
        "apt-cache",
        "policy",
        "(Package files|Pinned packages)",
        "APT package policy query",
    )
    output("apt-config", "dump", "^APT", "APT configuration decoding")
    output("apt-mark", "showmanual", "kinakaze-base", "APT manual package inventory")
    add(
        "dpkg",
        "mkdir -p jail/var/lib/dpkg; : > jail/var/lib/dpkg/status; "
        '"$1" --root="$PWD/jail" --force-not-root --force-bad-path --install fixture.deb; '
        "cmp ascii.txt jail/usr/share/audit-fixture; "
        '"$1" --root="$PWD/jail" --remove audit-fixture; test ! -e jail/usr/share/audit-fixture',
        "install, verify payload, and remove Debian package in private root",
    )
    output(
        "dpkg-query",
        "-W -f='${Package}\\n' kinakaze-base",
        "^kinakaze-base$",
        "installed package query",
    )
    output(
        "dpkg-deb",
        "--info fixture.deb",
        "Package: audit-fixture",
        "read Debian archive control metadata",
    )
    output(
        "apt-ftparchive",
        "packages .",
        "Package: audit-fixture",
        "generate Debian package index",
    )
    output(
        "apt-sortpkgs", "Packages", "Package: audit-fixture", "normalize package index"
    )
    output("dpkg-divert", "--list", ".*", "file diversion inventory")
    output(
        "dpkg-statoverride",
        "--list",
        ".*",
        "permission override inventory",
        codes=(0, 1),
    )
    output("dpkg-split", "--listq", ".*", "split-package queue inventory")
    output("dpkg-realpath", "text.txt", "/text.txt$", "dpkg path canonicalization")
    output(
        "update-alternatives",
        "--query editor",
        "Value: /bin/nano",
        "alternatives database query",
    )
    output("debconf-show", "kinakaze-base", ".*", "debconf package settings query")
    output(
        "debconf-communicate",
        "audit",
        "^0 2.0",
        "debconf protocol negotiation",
        "VERSION 2.0\n",
    )
    exact("debconf-escape", "-e", "a\\nb\\n", "debconf newline escaping", "a\nb\n")
    output("ucfq", "--with-colons", ".*", "configuration ownership inventory")
    add(
        "lcf",
        'code=0; "$1" "$PWD/text.txt" "$PWD" > out 2> err || code=$?; cat err; test "$code" = 2; grep -q "No record" err',
        "negative configuration-history lookup",
    )
    add(
        "ucf",
        'export DEBIAN_FRONTEND=noninteractive UCF_FORCE_CONFFNEW=1; "$1" --state-dir "$PWD" text.txt "$PWD/installed"; cmp text.txt installed',
        "configuration file registration in isolated state directory",
    )
    add(
        "ucfr",
        '"$1" --state-dir "$PWD" audit "$PWD/text.txt"; grep -q audit registry',
        "isolated configuration ownership registration",
    )
    output("man", "-P cat printf", "SYNOPSIS", "manual formatting pipeline")
    output("manpath", "", "/usr/share/man", "manual search path query")
    output("whatis apropos", "printf", "printf", "manual database lookup")
    output("lexgrog", "sample.1", "sample", "manual NAME section extraction")
    output("groff nroff", "-Tascii -man sample.1", "sample", "roff manual rendering")
    output("troff", "-Tascii -man sample.1", "sample", "roff intermediate output")
    output("grog", "sample.1", "groff", "infer roff preprocessing options")
    output("soelim", "include.roff", "sample", "roff include expansion")
    output("preconv", "-e UTF-8 sample.1", "sample", "roff input encoding conversion")
    output("tbl gtbl", "table.roff", "\\.ds|\\.de|alpha", "roff table preprocessing")
    output(
        "eqn geqn neqn",
        "equation.roff",
        "\\.lf|\\.EQ|\\.ds",
        "roff equation preprocessing",
    )
    output("pic gpic", "picture.roff", "\\.PS|\\.lf", "roff diagram preprocessing")
    output(
        "man-recode",
        "--to-code=UTF-8 --suffix=.utf8 sample.1; cat sample.1.utf8",
        "sample",
        "manual character encoding conversion",
    )
    output("col colcrt ul", "", "plain", "terminal formatting filter", "plain\n")
    exact("colrm", "2 3", "ade\n", "remove text columns", "abcde\n")
    output("column", "-t", "a[[:space:]]+b", "column alignment", "a b\nlong c\n")
    output("look", "alpha dictionary.txt", "^alpha$", "dictionary prefix search")
    exact("less pager", "ascii.txt", "hello\n", "non-terminal pagination output")
    exact("lessecho", "hello world", "hello world\n", "quote pager shell arguments")
    add(
        "lesskey",
        "-o compiled.less lesskey.txt; test -s compiled.less",
        "compile less key bindings",
    )
    add(
        "vim.tiny vi",
        "-e -s -V1 -u NONE -i NONE ascii.txt +'s/hello/edited/' +wq; grep -q edited ascii.txt",
        "batch text editing without terminal",
    )
    output("infocmp", "xterm", "xterm", "terminfo capability decoding")
    output("toe", "-a", "xterm", "terminfo database enumeration")
    output("tput", "-T xterm cols", "^[1-9][0-9]*$", "terminal column capability")
    add(
        "tic",
        'mkdir terminfo; "$1" -o terminfo terminal.src; test -n "$(find terminfo -type f)"',
        "compile terminfo description",
    )
    output("captoinfo", "termcap.txt", "audit", "termcap to terminfo conversion")
    output("infotocap", "terminal.src", "audit", "terminfo to termcap conversion")
    add("clear", '"$1" > out; test -s out', "emit terminal clear escape sequence")
    output(
        "gzip bzip2 xz",
        "-dc archive." + "{ext}",
        "hello",
        "decompress host-generated fixture",
    )
    # Substitute each compression format after the shared declarations.
    for name, extension in [("gzip", "gz"), ("bzip2", "bz2"), ("xz", "xz")]:
        result[name]["script"] = result[name]["script"].replace("{ext}", extension)
    for names, extension in [
        ("gunzip zcat uncompress", "gz"),
        ("bunzip2 bzcat", "bz2"),
        ("unxz xzcat", "xz"),
    ]:
        exact(names, "-c archive." + extension, "hello\n", "decompress known fixture")
    for names, extension in [
        ("zgrep zegrep zfgrep", "gz"),
        ("bzgrep bzegrep bzfgrep", "bz2"),
        ("xzgrep xzegrep xzfgrep", "xz"),
    ]:
        output(names, "hello archive." + extension, "^hello$", "search compressed text")
    for names, extension in [
        ("zcmp zdiff", "gz"),
        ("bzcmp bzdiff", "bz2"),
        ("xzcmp xzdiff", "xz"),
    ]:
        add(
            names,
            '"$1" archive.' + extension + " archive." + extension,
            "compare compressed files",
        )
    for names, extension in [
        ("zless zmore", "gz"),
        ("bzless bzmore", "bz2"),
        ("xzless xzmore", "xz"),
    ]:
        output(
            names,
            "archive." + extension,
            "hello",
            "display compressed text without terminal",
        )
    add(
        "zforce",
        'cp archive.gz compressed; "$1" compressed; test -f compressed.gz; gzip -dc compressed.gz > out; cmp ascii.txt out',
        "recognize and rename gzip data",
    )
    add(
        "gzexe bzexe",
        'cp script.sh program; chmod +x program; "$1" program; ./program > out; grep -q script-ok out',
        "compress and execute a shell program",
    )
    add(
        "zstd zstdmt pzstd",
        '"$1" -q -f ascii.txt -o sample.zst; "$1" -q -d -f sample.zst -o restored; cmp ascii.txt restored',
        "zstd compression roundtrip",
    )
    add(
        "unzstd zstdcat",
        'zstd -q -f ascii.txt -o sample.zst; "$1" -c sample.zst > restored; cmp ascii.txt restored',
        "zstd decompression",
    )
    output(
        "lzmainfo", "archive.lzma", "(Dictionary|Uncompressed)", "LZMA stream metadata"
    )
    add(
        "tar ptar",
        '-cf saved.tar ascii.txt; mkdir unpack; cd unpack; "$1" -xf ../saved.tar; cmp ascii.txt ../ascii.txt',
        "archive creation and extraction",
    )
    add(
        "cpio",
        'printf \'ascii.txt\\n\' | "$1" -o -H newc > saved.cpio; mkdir unpack; cd unpack; "$1" -id < ../saved.cpio; cmp ascii.txt ../ascii.txt',
        "cpio archive roundtrip",
    )
    add(
        "zip",
        '"$1" -q saved.zip ascii.txt; unzip -p saved.zip ascii.txt > out; cmp ascii.txt out',
        "ZIP creation with interoperable payload",
    )
    exact("unzip", "-p archive.zip ascii.txt", "hello\n", "ZIP extraction")
    exact("funzip", "archive.zip", "hello\n", "extract first ZIP member")
    output(
        "zipinfo zipdetails",
        "archive.zip",
        "ascii.txt",
        "ZIP directory/structure decoding",
    )
    output("zipgrep", "hello archive.zip", "hello", "search ZIP member content")
    output("zipnote", "archive.zip", "ascii.txt", "ZIP member comments")
    add("zipsplit", "-n 4096 archive.zip; test -f archive1.zip", "split ZIP archive")
    output(
        "perl perl5.36.0 perl5.36-x86_64-linux-gnu",
        "-MJSON::PP -e 'print encode_json({answer=>42})'",
        '"answer":42',
        "Perl module loading and JSON encoding",
    )
    exact(
        "json_pp",
        "-json_opt canonical",
        '{"answer":42}',
        "JSON parsing and serialization",
        '{"answer":42}',
    )
    output("corelist", "strict", "strict", "Perl core module inventory")
    output("encguess", "ascii.txt", "(ASCII|UTF)", "guess file encoding")
    output(
        "shasum",
        "text.txt",
        "^" + hashlib.sha1(b"alpha beta\nBeta gamma\nalpha beta\n").hexdigest(),
        "Perl digest command",
    )
    output("pod2text", "sample.pod", "audit", "POD to text conversion")
    output("pod2man", "sample.pod", "audit", "POD to man conversion")
    output(
        "pod2html",
        "--infile=sample.pod --outfile=sample.html; cat sample.html",
        "audit",
        "POD to HTML conversion",
    )
    add("podchecker", '"$1" sample.pod', "POD syntax validation")
    output("perldoc", "-T -F sample.pod", "audit", "render local Perl documentation")
    output("prove", "sample.t", "All tests successful", "run TAP test file")
    output(
        "perlbug perlthanks",
        "-d",
        "(perl|Perl)",
        "Perl diagnostic report without sending mail",
    )
    output("perlivp", "", "(ok|PASS)", "Perl installation self-check", codes=(0,))
    output(
        "splain",
        "",
        "syntax",
        "explain Perl diagnostic",
        'syntax error at - line 1, near "bad"\n',
    )
    add("pl2pm", '"$1" sample.pl; test -f Sample.pm', "convert Perl library to module")
    add(
        "h2xs",
        "-X -A -n Audit::Probe; test -f Audit-Probe/lib/Audit/Probe.pm",
        "generate Perl module scaffold",
    )
    add(
        "h2ph",
        'mkdir headers; "$1" -d "$PWD/headers" fixture.h; test -f headers/fixture.ph',
        "translate C header constants to Perl",
    )
    output(
        "findrule",
        ". -file -name text.txt",
        "text.txt",
        "Perl file predicate traversal",
    )
    output("ptargrep", "-l hello archive.tar", "ascii.txt", "search tar member content")
    add(
        "ptardiff",
        'mkdir compare; cp ascii.txt compare/; cd compare; "$1" ../archive.tar',
        "compare tar member to filesystem",
    )
    output(
        "python3 python3.11",
        '-c \'import sqlite3,ssl,json; print(json.dumps({"answer":sqlite3.connect(":memory:").execute("select 42").fetchone()[0]}))\'',
        '"answer": 42',
        "Python imports and SQLite query",
    )
    output("pydoc3 pydoc3.11", "json", "JSON", "Python module documentation")
    add(
        "py3compile",
        '"$1" sample.py; test -n "$(find __pycache__ -name "*.pyc")"',
        "compile Python source bytecode",
    )
    add(
        "py3clean",
        'python3 -m py_compile sample.py; "$1" .; test -z "$(find . -name "*.pyc")"',
        "remove Python bytecode caches",
    )
    output("py3versions", "-d", "python3.11", "Debian default Python version query")
    add(
        "pygettext3 pygettext3.11",
        "-o messages.pot sample.py; grep -q audit-message messages.pot",
        "extract Python translation strings",
    )
    output(
        "pdb3 pdb3.11",
        "-c continue sample.py",
        "python-ok",
        "Python debugger executes a script",
    )
    output(
        "chardet chardetect", "ascii.txt", "(ascii|ASCII)", "Python encoding detection"
    )
    output("normalizer", "ascii.txt", "(ascii|ASCII)", "Python charset analysis")
    output("systemd-escape", "hello/world", "hello-world", "systemd unit-name escaping")
    output("systemd-id128", "new", "^[0-9a-f]{32}$", "generate 128-bit identifier")
    output("systemd-path", "user-home", "/root", "systemd path lookup")
    output(
        "systemd-analyze",
        "calendar daily",
        "Normalized form:",
        "calendar expression parsing",
    )
    output("systemd-detect-virt", "", ".+", "virtualization detection", codes=(0, 1))
    output(
        "systemctl",
        "is-system-running",
        "^running$|^degraded$",
        "query service manager runtime state",
        codes=(0,),
    )
    output(
        "loginctl",
        "list-sessions",
        "(SESSION|sessions)",
        "query login manager sessions",
    )
    output("hostnamectl", "status", "hostname", "query host service properties")
    output("localectl", "status", "Locale", "query locale service properties")
    output("timedatectl", "status", "time", "query time service properties")
    output("networkctl", "list", "(IDX|LINK)", "query network manager links")
    output("journalctl", "--no-pager -n 1", ".*", "read journal records")
    output("systemd-cgls", "--no-pager", ".+", "cgroup tree query")
    output("systemd-delta", "--no-pager", ".+", "systemd configuration override query")
    output("busctl", "--system list", "(NAME|org.)", "query system bus names")
    output("dbus-uuidgen", "", "^[0-9a-f]{32}$", "generate D-Bus machine identifier")
    output(
        "dbus-run-session",
        "-- /usr/bin/printf dbus-session-ok",
        "dbus-session-ok",
        "launch private session bus",
    )
    add(
        "systemd-tmpfiles",
        '--create --root="$PWD/jail" audit.conf; test -d jail/tmp/audit-created',
        "create directory from isolated tmpfiles rule",
    )
    add(
        "systemd-sysusers",
        '--root="$PWD/jail" audit.conf; grep -q auditdaemon jail/etc/passwd',
        "create account from isolated sysusers rule",
    )
    add(
        "systemd-machine-id-setup",
        '--root="$PWD/jail"; test -s jail/etc/machine-id',
        "initialize isolated machine identity",
    )
    add(
        "systemd-firstboot",
        '--root="$PWD/jail" --hostname=audit-host; grep -q audit-host jail/etc/hostname',
        "initialize isolated hostname",
    )
    output(
        "udevadm",
        "info --query=all --path=/sys/class/net/lo",
        "INTERFACE=lo",
        "udev network-device metadata",
    )
    output("kmod", "list", "(Module|Size)", "loaded kernel module listing")
    output("lsmod", "", "(Module|Size)", "loaded kernel module listing")
    output("modinfo", "loop", "(filename|name):", "kernel module metadata")
    output("dmesg", "--notime", ".+", "read kernel log")
    output("lspci", "-n", "[0-9a-f]{2}:[0-9a-f]{2}", "PCI device enumeration")
    output(
        "dmidecode",
        "-t system",
        "(System Information|Manufacturer)",
        "firmware system information",
    )
    output("hwclock", "--show", "[0-9]{4}-", "hardware clock query")
    output(
        "logrotate", "-d logrotate.conf", ".*", "dry-run log rotation policy parsing"
    )
    output("validlocale", "C.UTF-8", ".*", "locale-name validation")
    output("zdump", "UTC", "UTC", "timezone formatting")
    add(
        "zic",
        "-d zoneinfo zone.txt; test -f zoneinfo/Audit/UTC",
        "compile timezone definition",
    )
    output("uuidgen", "", "^[0-9a-f-]{36}$", "UUID generation")
    output(
        "uuidparse",
        "12345678-1234-4234-8234-123456789abc",
        "12345678",
        "UUID field decoding",
    )
    output("mcookie", "", "^[0-9a-f]{32}$", "random authentication cookie")
    add(
        "xauth",
        '-f authority add localhost:0 . 0123456789abcdef0123456789abcdef; "$1" -f authority list > out; grep -q 0123456789abcdef out',
        "isolated X authority database",
    )
    for name, operation, verify in [
        ("useradd", "-M audituser", "grep -q ^audituser: jail/etc/passwd"),
        ("userdel", "fixture", "! grep -q ^fixture: jail/etc/passwd"),
        ("usermod", "-c changed fixture", "grep -q changed jail/etc/passwd"),
        ("groupadd", "auditgroup", "grep -q ^auditgroup: jail/etc/group"),
        ("groupdel", "unused", "! grep -q ^unused: jail/etc/group"),
        ("groupmod", "-n changed fixture", "grep -q ^changed: jail/etc/group"),
    ]:
        add(
            name,
            '--prefix "$PWD/jail" ' + operation + "; " + verify,
            "account database operation confined to private prefix",
        )
    output(
        "pwck", '-r -R "$PWD/jail"', ".*", "read-only passwd consistency", codes=(0, 2)
    )
    output(
        "grpck", '-r -R "$PWD/jail"', ".*", "read-only group consistency", codes=(0, 2)
    )
    output(
        "policy-rc.d",
        "audit start",
        ".*",
        "preinstalled service-start policy",
        codes=(101,),
    )
    output(
        "nologin", "", "(available|login)", "deny login with nonzero status", codes=(1,)
    )
    for name in ("fdisk", "sfdisk"):
        output(
            name, "-l disk.img", "(Disk|disk.img)", "inspect isolated blank disk image"
        )
    output(
        "blkid",
        "-p disk.img",
        ".*",
        "recognize absence of filesystem signature",
        codes=(2,),
    )
    output("wipefs", "-n disk.img", ".*", "read-only filesystem signature scan")
    output(
        "partx",
        "--show disk.img",
        ".*",
        "partition-table query on isolated image",
        codes=(0, 1),
    )
    output(
        "fsck",
        "-N disk.img",
        "fsck",
        "show filesystem-check invocation without execution",
    )
    add(
        "mke2fs mkfs.ext2 mkfs.ext3 mkfs.ext4",
        "-q -F disk.img; blkid -p disk.img > out; grep -q ext out",
        "create filesystem in isolated regular file",
    )
    add(
        "mkswap",
        "disk.img; blkid -p disk.img > out; grep -q swap out",
        "create swap signature in isolated regular file",
    )
    add(
        "mkfs.minix",
        "disk.img; blkid -p disk.img > out; grep -q minix out",
        "create Minix filesystem in isolated regular file",
    )
    add(
        "mkfs.cramfs",
        "sourcefs image.cramfs; test -s image.cramfs",
        "create compressed filesystem from fixture directory",
    )
    add(
        "mkfs.bfs",
        "disk.img; test -s disk.img",
        "create BFS filesystem in isolated regular file",
    )
    # Disk query utilities run against a filesystem prepared inside this case.
    for name, options, pattern in [
        ("dumpe2fs", "-h", "Filesystem"),
        ("e2label", "", "auditfs"),
        ("tune2fs", "-l", "Filesystem"),
        ("e2fsck", "-fn", "auditfs"),
        ("fsck.ext2", "-fn", "auditfs"),
        ("fsck.ext3", "-fn", "auditfs"),
        ("fsck.ext4", "-fn", "auditfs"),
        ("e2freefrag", "", "Device:"),
        ("debugfs", "-R stats", "Filesystem"),
    ]:
        add(
            name,
            'mke2fs -q -F -t ext2 -L auditfs disk.img; "$1" '
            + options
            + " disk.img > out 2> err; cat out; grep -E "
            + shlex.quote(pattern)
            + " out",
            "inspect/check isolated ext2 image",
        )
    add(
        "badblocks",
        "-b 4096 disk.img; test -s disk.img",
        "read-only bad-block scan of isolated regular file",
    )
    output("more", "ascii.txt", "^hello$", "non-terminal pagination with file heading")
    add(
        "pod2usage",
        'code=0; "$1" sample.pod > out 2>&1 || code=$?; cat out; test "$code" = 2; grep -q audit-run out',
        "POD usage text and documented exit status",
    )
    add(
        "bzip2recover",
        '"$1" archive.bz2; bzip2 -dc rec00001archive.bz2 > out; cmp ascii.txt out',
        "recover bzip2 blocks and verify payload",
    )
    add(
        "zstdgrep",
        'zstd -q ascii.txt -o sample.zst; "$1" hello sample.zst > out; grep -q hello out',
        "search zstd-compressed content",
    )
    add(
        "zstdless",
        'zstd -q ascii.txt -o sample.zst; "$1" sample.zst > out; grep -q hello out',
        "display zstd content without terminal",
    )
    output("lesspipe", "archive.gz", "hello", "pager input preprocessing")
    add(
        "lessfile",
        'p=$("$1" archive.gz); cat "$p" > out; cmp ascii.txt out; rm "$p"',
        "pager preprocessing into temporary file",
    )
    add(
        "streamzip",
        '"$1" -zipfile saved.zip < ascii.txt; unzip -p saved.zip > out; cmp ascii.txt out',
        "stream input into ZIP archive",
    )
    output("pidof", "bash", "[0-9]+", "lookup current shell process by name")
    output("fuser", ".", "[0-9]+", "find process using current directory")
    output("pslog", "$$", ".+", "process log-file query", codes=(0,))
    output("renice", "-n 1 -p $$", "priority", "change isolated shell niceness")
    output("choom", "-p $$", "score", "query OOM score")
    output("uclampset", "-p $$", ".+", "query utilization clamp state")
    output(
        "setarch",
        "x86_64 uname -m",
        "x86_64",
        "run command with architecture personality",
    )
    output("x86_64 linux64", "uname -m", "x86_64", "64-bit execution personality")
    output(
        "i386 linux32",
        "uname -m",
        "i[3-6]86",
        "32-bit reported architecture personality",
    )
    output("su", '-s /bin/sh -c "id -u" root', "^0$", "run shell as root through su")
    output("runuser", "-u root -- id -u", "^0$", "run command under root account")
    output("sg", 'root -c "id -g"', "^0$", "run command under root group")
    output(
        "start-stop-daemon",
        "--test --start --exec /bin/true",
        ".+",
        "dry-run daemon launch decision",
    )
    output("ifquery", "--list", "lo", "parse configured network interfaces")
    output("rtacct", "", ".+", "routing accounting query")
    output("routel", "", ".+", "formatted routing table")
    output("vdpa", "dev show", ".*", "virtual datapath device query")
    output("zramctl", "", ".*", "compressed RAM block-device query")
    output("ldconfig", "-p", "(Cache|cache|libc)", "shared library cache lookup")
    output("ldd", "/bin/cat", "libc", "executable shared-library dependency listing")
    add(
        "fstab-decode",
        '"$1" /usr/bin/printf "%s" "hello\\040world" > out; test "$(cat out)" = "hello world"',
        "decode fstab octal escaping in child argument",
    )
    add(
        "logsave",
        '"$1" saved.log /usr/bin/printf logsave-ok; grep -q logsave-ok saved.log',
        "capture command output in log file",
    )
    add(
        "mkfs",
        "-t ext2 -q -F disk.img; blkid -p disk.img > out; grep -q ext2 out",
        "generic filesystem creation dispatcher on regular file",
    )
    add(
        "e2image",
        'mke2fs -q -F disk.img; "$1" disk.img metadata.img; test -s metadata.img',
        "export ext filesystem metadata image",
    )
    add(
        "resize2fs",
        'mke2fs -q -F disk.img; truncate -s 24M disk.img; "$1" disk.img; e2fsck -fn disk.img',
        "grow isolated ext filesystem image",
    )
    add(
        "fsck.minix",
        'mkfs.minix disk.img; "$1" -f disk.img',
        "check isolated Minix filesystem",
    )
    add(
        "fsck.cramfs",
        'mkfs.cramfs sourcefs image.cramfs; "$1" image.cramfs',
        "check isolated cramfs image",
    )
    add(
        "swaplabel",
        'mkswap disk.img; "$1" -L audit-swap disk.img; "$1" disk.img > out; grep -q audit-swap out',
        "update and read swap label in regular file",
    )
    add(
        "mklost+found",
        '"$1"; test -d lost+found',
        "create lost+found in isolated directory",
    )
    add(
        "setcap",
        '"$1" cap_net_bind_service=ep ascii.txt; getcap ascii.txt > out; grep -q cap_net_bind_service out',
        "file capability roundtrip on isolated file",
    )
    add(
        "chattr",
        '"$1" +d ascii.txt; lsattr ascii.txt > out; grep -q d out',
        "set isolated file nodump attribute",
    )
    add(
        "dpkg-maintscript-helper",
        'export DPKG_MAINTSCRIPT_NAME=postinst DPKG_MAINTSCRIPT_PACKAGE=audit; "$1" supports rm_conffile',
        "maintainer-helper capability query",
    )
    output("dpkg-trigger", "--check-supported", ".*", "dpkg trigger support query")
    output(
        "debconf", "/bin/true", ".*", "run command with debconf protocol environment"
    )
    add(
        "debconf-set-selections",
        'printf "audit audit/question string audit-value\\n" | "$1" --checkonly',
        "validate debconf selections without changing database",
    )
    output("dpkg-preconfigure", "--apt", ".*", "empty APT preconfiguration input")
    output(
        "apt-listchanges",
        "--which=changelogs --frontend=text fixture.deb",
        ".*",
        "inspect changelog from local Debian archive",
    )
    output(
        "instmodsh",
        "",
        "Installed modules",
        "Perl installed-module query followed by explicit quit",
        "l\nq\n",
    )
    add(
        "sensible-editor",
        'export EDITOR=/bin/true VISUAL=/bin/true; "$1" ascii.txt',
        "dispatch explicitly configured editor",
    )
    output(
        "sensible-pager", "ascii.txt", "hello", "pager dispatch for non-terminal output"
    )
    add(
        "savelog",
        'cp ascii.txt audit.log; "$1" -c 2 -l audit.log; cmp ascii.txt audit.log.0',
        "rotate isolated log while preserving payload",
    )
    add(
        "dotlockfile",
        '"$1" -l -r 0 lock; test -f lock; "$1" -u lock; test ! -e lock',
        "dotlock creation and removal",
    )
    add(
        "c_rehash",
        'mkdir certs; "$1" certs',
        "rehash isolated empty certificate directory",
    )
    add(
        "mandb",
        'mkdir -p manuals/man1; cp sample.1 manuals/man1/; "$1" -c manuals; test -s manuals/index.db',
        "build manual index from isolated fixture",
    )
    output(
        "tasksel",
        "--list-tasks",
        "(standard|ssh-server|desktop)",
        "installed task definitions query",
    )
    output("pam_getenv", "PATH", ".*", "read PAM environment value")
    add(
        "ssh-agent",
        '"$1" /bin/sh -c \'test -S "$SSH_AUTH_SOCK"; echo agent-ok\' > out; grep -q agent-ok out',
        "SSH agent socket lifecycle",
    )
    add(
        "ssh-add",
        "ssh-keygen -q -t ed25519 -N \"\" -f key; ssh-agent /bin/sh -c 'ssh-add key; ssh-add -l' > out; grep -q ED25519 out",
        "load and list key using temporary SSH agent",
    )
    output(
        "script",
        '-q -e -c "printf pty-ok" transcript',
        "pty-ok",
        "run command through pseudo-terminal and save transcript",
    )
    add(
        "stty",
        'script -q -e -c ""$1" -a" transcript > out; grep -q speed out',
        "query pseudo-terminal attributes",
    )
    add(
        "tty",
        'script -q -e -c ""$1"" transcript > out; grep -q /dev/ out',
        "resolve pseudo-terminal name",
    )
    output(
        "ischroot",
        "",
        ".*",
        "detect chroot state with documented status",
        codes=(0, 1, 2),
    )
    # Prefix every direct-argument fragment with the actual command path.
    for case in result.values():
        if case["script"].startswith(("-", "disk.img;", "sourcefs ")):
            case["script"] = '"$1" ' + case["script"]
    return result


def prepare(root: Path, case):
    fixtures = {
        "text.txt": "alpha beta\nBeta gamma\nalpha beta\n",
        "ascii.txt": "hello\n",
        "lines.txt": "one\ntwo\nthree\n",
        "left.txt": "a\nb\n",
        "same.txt": "a\nb\n",
        "right.txt": "1\n2\n",
        "join1.txt": "a one\nb two\n",
        "join2.txt": "a 1\nb 2\n",
        "dictionary.txt": "alpha\nbeta\ngamma\n",
        "script.sh": "#!/bin/sh\necho script-ok\n",
        "sample.1": ".TH SAMPLE 1\n.SH NAME\nsample \\- audit command\n.SH DESCRIPTION\nhello world\n",
        "include.roff": ".so sample.1\n",
        "table.roff": ".TS\nl l.\nalpha\tbeta\n.TE\n",
        "equation.roff": ".EQ\nx = y sup 2\n.EN\n",
        "picture.roff": '.PS\nbox "audit"\n.PE\n',
        "sample.pod": "=head1 NAME\n\naudit - test documentation\n\n=head1 SYNOPSIS\n\naudit-run\n\n=head1 DESCRIPTION\n\nhello\n\n=cut\n",
        "sample.py": 'def _(s): return s\nprint("python-ok")\n_("audit-message")\n',
        "sample.pl": "package sample; sub hello { return 42; } 1;\n",
        "sample.t": 'print "1..1\\nok 1 - audit\\n";\n',
        "fixture.h": "#define AUDIT_VALUE 42\n",
        "lesskey.txt": "#command\nq quit\n",
        "terminal.src": "audit|audit terminal,\n cols#80, lines#24, clear=\\E[H\\E[2J,\n",
        "termcap.txt": "audit|audit terminal:co#80:li#24:cl=\\E[H\\E[2J:\n",
        "logrotate.conf": str("/tmp/audit-no-such.log")
        + " {\n missingok\n rotate 1\n}\n",
        "zone.txt": "Zone Audit/UTC 0 - UTC\n",
        "tmpfiles.conf": "d /tmp/audit-created 0755 root root -\n",
        "sysusers.conf": 'u auditdaemon - "Audit daemon" / /bin/false\n',
        "Packages": "Package: audit-fixture\nVersion: 1\nArchitecture: amd64\nDescription: audit\n\n",
    }
    for name, text in fixtures.items():
        (root / name).write_text(text, encoding="utf-8", newline="\n")
    (root / "bytes.bin").write_bytes(bytes((0, 1, 127, 255)))
    data = b"hello\n"
    for suffix, payload in [
        ("gz", gzip.compress(data)),
        ("bz2", bz2.compress(data)),
        ("xz", lzma.compress(data)),
        ("lzma", lzma.compress(data, format=lzma.FORMAT_ALONE)),
    ]:
        (root / ("archive." + suffix)).write_bytes(payload)
    with zipfile.ZipFile(root / "archive.zip", "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("ascii.txt", data)
    with tarfile.open(root / "archive.tar", "w") as archive:
        member = tarfile.TarInfo("ascii.txt")
        member.size = len(data)
        member.mode = 0o644
        archive.addfile(member, io.BytesIO(data))
    control = b"Package: audit-fixture\nVersion: 1\nArchitecture: amd64\nMaintainer: Audit <audit@example.invalid>\nDescription: Audit fixture\n"

    def tar(name, data):
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode="w") as archive:
            for parent in reversed(PurePosixPath(name).parents):
                member = tarfile.TarInfo(str(parent))
                member.type = tarfile.DIRTYPE
                member.mode = 0o755
                archive.addfile(member)
            member = tarfile.TarInfo(name)
            member.size = len(data)
            member.mode = 0o644
            archive.addfile(member, io.BytesIO(data))
        return out.getvalue()

    deb = b"!<arch>\n"
    for name, payload in [
        ("debian-binary", b"2.0\n"),
        ("control.tar", tar("./control", control)),
        ("data.tar", tar("./usr/share/audit-fixture", data)),
    ]:
        header = f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(payload):<10}`\n".encode()
        deb += header + payload + (b"\n" if len(payload) % 2 else b"")
    (root / "fixture.deb").write_bytes(deb)
    if "disk.img" in case["script"]:
        with (root / "disk.img").open("wb") as stream:
            stream.truncate(16 * 1024 * 1024)
    (root / "sourcefs").mkdir(exist_ok=True)
    (root / "sourcefs/hello").write_bytes(data)
    if "jail" in case["script"]:
        for name in ("etc", "tmp", "var/log", "root", "home/fixture"):
            (root / "jail" / name).mkdir(parents=True, exist_ok=True)
        for name, text in {
            "passwd": "root:x:0:0:root:/root:/bin/sh\nfixture:x:1500:1500:Fixture:/home/fixture:/bin/sh\n",
            "group": "root:x:0:\nfixture:x:1500:\nunused:x:1501:\n",
            "shadow": "root:!:19000:0:99999:7:::\nfixture:!:19000:0:99999:7:::\n",
            "gshadow": "root:!::\nfixture:!::\n",
            "login.defs": "UID_MIN 1000\nUID_MAX 60000\nGID_MIN 1000\nGID_MAX 60000\n",
        }.items():
            (root / "jail/etc" / name).write_text(text, encoding="utf-8", newline="\n")
        for directory, fixture in [("tmpfiles.d", "tmpfiles.conf"), ("sysusers.d", "sysusers.conf")]:
            target = root / "jail/usr/lib" / directory / "audit.conf"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(fixtures[fixture], encoding="utf-8", newline="\n")
