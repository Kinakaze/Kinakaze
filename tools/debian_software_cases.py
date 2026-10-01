"""Real workflows extending the standard command catalog to application packages."""


def cases():
    result = {}

    def add(names, script, purpose, timeout=30):
        for name in names.split():
            result[name] = dict(script=script, purpose=purpose, timeout=timeout)

    add(
        "gcc gcc-12 cc clang clang-14 clang-15 clang-16",
        "printf '#include <stdio.h>\\nint main(void){puts(\"compiler-ok\");return 0;}\\n' > main.c; "
        '"$1" main.c -o program; ./program > out; grep -qx compiler-ok out',
        "compile, link and execute a C program",
        60,
    )
    add(
        "g++ g++-12 c++ clang++ clang++-14 clang++-15 clang++-16",
        "printf '#include <iostream>\\n#include <thread>\\nint main(){int v=0;std::thread t([&]{v=42;});t.join();std::cout<<v<<std::endl;}\\n' > main.cpp; "
        '"$1" -std=c++17 main.cpp -pthread -o program; ./program > out; grep -qx 42 out',
        "compile and execute a C++ thread with libstdc++",
        60,
    )
    add(
        "git",
        '''"$1" init -q -b main repository
"$1" -C repository config user.name Compatibility
"$1" -C repository config user.email compatibility@example.invalid
printf 'first\n' > repository/payload
"$1" -C repository add payload
"$1" -C repository commit -qm first
"$1" -C repository checkout -qb feature
printf 'second\n' >> repository/payload
"$1" -C repository commit -qam second
"$1" -C repository checkout -q main
"$1" -C repository merge --ff-only feature
"$1" -C repository gc --prune=now
"$1" -C repository fsck --strict
"$1" clone --no-hardlinks repository clone
cmp repository/payload clone/payload
test "$("$1" -C clone rev-list --count HEAD)" = 2
test -z "$("$1" -C clone status --porcelain)"''',
        "commit, branch, merge, garbage collect, verify and clone a repository",
        90,
    )
    add(
        "make gmake",
        "printf 'all: result\\nresult: ascii.txt\\n\\tcp ascii.txt result\\n' > Makefile; "
        '"$1" -j2; cmp ascii.txt result; "$1" -q',
        "parallel Make dependency execution",
    )
    add(
        "cmake",
        """printf 'cmake_minimum_required(VERSION 3.16)\nproject(probe C)\nadd_executable(probe main.c)\n' > CMakeLists.txt
printf '#include <stdio.h>\nint main(void){puts("cmake-ok");return 0;}\n' > main.c
"$1" -S . -B build
"$1" --build build --parallel 2
./build/probe > out; grep -qx cmake-ok out""",
        "configure, compile and run a CMake project",
        90,
    )
    add(
        "ninja",
        "printf 'rule copy\\n  command = cp $in $out\\nbuild copied: copy ascii.txt\\n' > build.ninja; "
        '"$1" -j2; cmp ascii.txt copied; "$1" -n > out; grep -q "no work to do" out',
        "Ninja dependency graph and incremental build",
    )
    add(
        "node nodejs",
        """"$1" -e 'const fs=require("fs"),cp=require("child_process"),assert=require("assert");
fs.writeFileSync("node-payload","hello");assert.equal(fs.readFileSync("node-payload","utf8"),"hello");
assert.equal(cp.execFileSync("/bin/echo",["child"],{encoding:"utf8"}),"child\\n");
Promise.all(Array.from({length:8},(_,i)=>fs.promises.writeFile("node-"+i,"value"))).then(()=>console.log("node-ok"));' > out
grep -qx node-ok out; test -f node-7""",
        "Node file IO, promises and child process output",
        60,
    )
    add(
        "ruby ruby3.1",
        """"$1" -e 'require "json"; require "open3"; require "thread";
raise unless JSON.parse(JSON.generate({"v"=>42}))["v"]==42;
out,status=Open3.capture2("/bin/echo","child");raise unless status.success? && out=="child\\n";
raise unless (0...8).map{|i| Thread.new{i*i}}.map(&:value).sum==140;puts "ruby-ok"' > out
grep -qx ruby-ok out""",
        "Ruby JSON, threads and subprocess capture",
        60,
    )
    add(
        "php php8.2",
        """"$1" -r '$v=json_decode(json_encode(["v"=>42]),true); if($v["v"]!==42)exit(1);
file_put_contents("php-file","hello");if(file_get_contents("php-file")!=="hello")exit(2);
echo "php-ok\\n";' > out; grep -qx php-ok out""",
        "PHP JSON and file IO",
    )
    add(
        "sqlite3",
        '''"$1" database 'create table t(v);begin;insert into t values(42);commit;begin;insert into t values(99);rollback;'
test "$("$1" database 'select sum(v) from t;pragma integrity_check;')" = "$(printf '42\nok')"''',
        "SQLite transaction, rollback, reopen and integrity check",
    )
    add(
        "rsync",
        'mkdir source copied; cp ascii.txt source/payload; "$1" -a source/ copied/; '
        "cmp source/payload copied/payload; printf changed > source/payload; "
        '"$1" -a --checksum source/ copied/; cmp source/payload copied/payload',
        "rsync archive copy and checksum based update",
    )
    add(
        "patch",
        "printf 'hello\\n' > original; printf '%s\\n' '--- original' '+++ original' '@@ -1 +1 @@' '-hello' '+patched' > change.patch; "
        '"$1" -p0 < change.patch; grep -qx patched original; "$1" -R -p0 < change.patch; grep -qx hello original',
        "apply and reverse a unified patch",
    )
    add(
        "bc",
        "printf 'scale=3; 22/7\\n2^10\\n' | \"$1\" > out; "
        "printf '3.142\\n1024\\n' > expected; cmp out expected",
        "arbitrary precision division and power",
    )
    add(
        "dc",
        "printf '2 10 ^ p\\n' | \"$1\" > out; grep -qx 1024 out",
        "stack based arithmetic",
    )
    add(
        "jq",
        'printf \'%s\' \'{"items":[1,2,3],"label":"hello"}\' | "$1" -r \'.items|add\' > out; '
        "grep -qx 6 out",
        "JSON parsing, array traversal and reduction",
    )
    add(
        "gawk",
        "printf 'alpha 2\\nbeta 3\\n' | \"$1\" '{total += $2} END {print total}' > out; grep -qx 5 out",
        "Awk field parsing and arithmetic aggregation",
    )
    add(
        "rg",
        '"$1" -n --fixed-strings "alpha beta" text.txt > out; '
        "printf '1:alpha beta\\n3:alpha beta\\n' > expected; cmp out expected",
        "ripgrep exact matches and line numbers",
    )
    add(
        "fd fdfind",
        "mkdir -p tree/sub; cp ascii.txt tree/sub/payload.txt; "
        '"$1" --color never --type f payload tree > out; grep -q tree/sub/payload.txt out',
        "recursive filename and file type filtering",
    )
    add(
        "tree",
        'mkdir -p tree/sub; cp ascii.txt tree/sub/payload; "$1" -i tree > out; grep -qx payload out',
        "recursive directory listing",
    )
    add(
        "xmlstarlet",
        "printf '<root><value>42</value></root>' > input.xml; "
        '"$1" sel -t -v /root/value input.xml > out; test "$(cat out)" = 42',
        "XML XPath extraction",
    )
    add(
        "xmllint",
        "printf '<root><value>42</value></root>' > input.xml; "
        '"$1" --xpath "string(/root/value)" input.xml > out; test "$(cat out)" = 42',
        "XML parse and XPath extraction",
    )
    add(
        "file",
        '"$1" ascii.txt > out; grep -q "ASCII text" out; "$1" bytes.bin > out; grep -q data out',
        "text and binary magic identification",
    )
    add(
        "pkg-config pkgconf",
        """mkdir pc
printf 'prefix=/probe\nName: fixture\nDescription: fixture\nVersion: 1.2.3\nLibs: -L${prefix}/lib -lfixture\nCflags: -I${prefix}/include\n' > pc/fixture.pc
export PKG_CONFIG_LIBDIR="$PWD/pc"
test "$("$1" --modversion fixture)" = 1.2.3
"$1" --libs --cflags fixture > out; grep -q -- -lfixture out; grep -q -- -I/probe/include out""",
        "pkg-config metadata, version and compiler flags",
    )
    add(
        "crontab",
        """export USER=root LOGNAME=root
"$1" -l > previous 2> previous.err || true
restore() { if test -s previous; then "$1" previous; else "$1" -r || true; fi; }
trap 'restore "$1"' EXIT
printf '17 3 * * * /bin/true\n' > schedule
"$1" schedule; "$1" -l > out; cmp schedule out""",
        "install, read and restore isolated guest crontab",
    )
    add(
        "sudo",
        '"$1" -n -u root /usr/bin/id -u > out; grep -qx 0 out',
        "sudo identity switch and command execution",
    )
    add(
        "lsb_release",
        '"$1" -ds > out; expected=$(. /etc/os-release; printf "%s" "$PRETTY_NAME"); grep -F -- "$expected" out',
        "distribution identity from OS metadata",
    )
    add(
        "curl-config",
        '"$1" --protocols > out; grep -qx HTTP out; grep -qx HTTPS out',
        "query compiled curl protocol support",
    )
    add(
        "cpp cpp-12 x86_64-linux-gnu-cpp x86_64-linux-gnu-cpp-12",
        "printf '#define VALUE 42\\nVALUE\\n' > preprocess.c; "
        '"$1" -P preprocess.c > out; grep -qx 42 out',
        "C macro preprocessing",
    )
    add(
        "ar x86_64-linux-gnu-ar",
        '"$1" rcs library.a ascii.txt; "$1" t library.a > out; grep -qx ascii.txt out; '
        'mkdir extracted; cd extracted; "$1" x ../library.a; cmp ascii.txt ../ascii.txt',
        "archive create, index, enumerate and extract",
    )
    add(
        "nm x86_64-linux-gnu-nm",
        "printf 'int software_value(void){return 42;}\\n' > symbol.c; gcc -c symbol.c -o symbol.o; "
        '"$1" -g symbol.o > out; grep -q " T software_value" out',
        "ELF symbol decoding",
    )
    add(
        "readelf x86_64-linux-gnu-readelf",
        "printf 'int software_value(void){return 42;}\\n' > symbol.c; gcc -c symbol.c -o symbol.o; "
        '"$1" -h symbol.o > out; grep -q ELF64 out; "$1" -s symbol.o > out; grep -q software_value out',
        "ELF header and symbol table decoding",
    )
    add(
        "objdump x86_64-linux-gnu-objdump",
        "printf 'int software_value(void){return 42;}\\n' > symbol.c; gcc -c symbol.c -o symbol.o; "
        '"$1" -d symbol.o > out; grep -q "<software_value>:" out',
        "decode executable machine code",
    )
    add(
        "strings x86_64-linux-gnu-strings",
        "printf '\\000software-string\\000\\377' > data; \"$1\" data > out; grep -qx software-string out",
        "extract a known string from binary data",
    )
    add(
        "c++filt x86_64-linux-gnu-c++filt",
        'test "$("$1" _Z5helloi)" = "hello(int)"',
        "C++ symbol demangling",
    )
    add(
        "gencat",
        "printf '$set 1\\n1 software-message\\n' > message.msg; \"$1\" catalog.cat message.msg; "
        'printf \'#include <nl_types.h>\\n#include <stdio.h>\\nint main(void){nl_catd c=catopen("./catalog.cat",0);puts(catgets(c,1,1,"missing"));return catclose(c);}\\n\' > message.c; '
        "gcc message.c -o message; ./message > out; grep -qx software-message out",
        "compile a message catalog and read it through libc catgets",
        60,
    )
    add(
        "rake",
        'printf \'task :default do\\n File.write("rake-result","rake-ok")\\nend\\n\' > Rakefile; '
        '"$1"; test "$(cat rake-result)" = rake-ok',
        "execute a Ruby Rake task",
    )
    add(
        "vim.basic vim",
        'printf "hello\\n" > edit; "$1" -e -s -u NONE -i NONE edit +"s/hello/edited/" +wq; '
        "grep -qx edited edit",
        "Vim batch editing and save",
    )
    add(
        "update-passwd",
        '"$1" --dry-run > out',
        "validate system account database against master records",
    )
    add(
        "invoke-rc.d",
        'code=0; "$1" --query cron start > out 2> err || code=$?; test "$code" = 101',
        "service policy denies automatic startup in the isolated guest",
    )
    add(
        "update-ca-certificates",
        """mkdir certs local output hooks
certificate=$(find /usr/share/ca-certificates -name '*.crt' -print -quit)
test -n "$certificate"; cp "$certificate" certs/audit.crt; printf 'audit.crt\n' > cert.conf
"$1" --fresh --certsconf "$PWD/cert.conf" --certsdir "$PWD/certs" \
 --localcertsdir "$PWD/local" --etccertsdir "$PWD/output" --hooksdir "$PWD/hooks"
test -s output/ca-certificates.crt; openssl verify -CAfile output/ca-certificates.crt certs/audit.crt > out
grep -q ': OK' out""",
        "build an isolated CA bundle and verify its included certificate",
        60,
    )
    add(
        "exim exim4",
        'test "$("$1" -be \'${eval:6*7}\')" = 42',
        "Exim runtime expansion engine",
    )
    add(
        "fc-pattern",
        '"$1" -f "%{family}\\n%{size}\\n" "SoftwareFixture:size=13" > out; '
        "printf 'SoftwareFixture\\n13\\n' > expected; cmp out expected",
        "Fontconfig pattern parsing and formatted properties",
    )
    add(
        "update-mime-database",
        """mkdir -p mime/packages
printf '%s' '<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info"><mime-type type="application/x-kinakaze-fixture"><comment>Software fixture</comment><glob pattern="*.kinakaze-fixture"/></mime-type></mime-info>' > mime/packages/fixture.xml
"$1" "$PWD/mime"
test -s mime/mime.cache; test -s mime/application/x-kinakaze-fixture.xml
grep -q 'application/x-kinakaze-fixture' mime/globs""",
        "compile isolated MIME XML definitions into lookup caches",
    )
    add(
        "dbilogstrip",
        "printf 'DBI::db=HASH(0x1234abcd) pid#123 thr#456\\n' | \"$1\" > out; "
        "printf 'DBI::db=HASH(0xN) pidN thrN\\n' > expected; cmp out expected",
        "normalize DBI trace addresses and process identifiers",
    )
    add(
        "adduser",
        """name=compat500_$$
cleanup() { deluser "$name" >/dev/null 2>&1 || true; delgroup "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
"$1" --system --group --no-create-home --home /nonexistent --shell /usr/sbin/nologin "$name"
id "$name" > out; getent passwd "$name" > account; grep -q /usr/sbin/nologin account
cleanup; if getent passwd "$name"; then exit 1; fi; if getent group "$name"; then exit 1; fi""",
        "create, query and remove an isolated system account and group",
    )
    add(
        "socat",
        '"$1" -u OPEN:ascii.txt OPEN:copied,creat,trunc; cmp ascii.txt copied',
        "socat transfer between two real file descriptors",
    )
    add(
        "HEAD",
        '"$1" -m GET "$AUDIT_HTTP_URL" > out; grep -qx kinakaze-http-ok out',
        "Perl LWP client performs an actual loopback HTTP request",
    )
    add(
        "run-mailcap",
        "printf 'text/plain; cat %%s\\n' > fixture.mailcap; export MAILCAPS=\"$PWD/fixture.mailcap\"; "
        '"$1" --action=view text/plain:ascii.txt > out; cmp ascii.txt out',
        "parse isolated mailcap and execute its MIME handler",
    )
    add(
        "my_print_defaults",
        "printf '[fixture]\\nport=12345\\nsocket=/tmp/software.sock\\n' > client.cnf; "
        '"$1" --defaults-file="$PWD/client.cnf" fixture > out; '
        "printf '%s\\n' '--port=12345' '--socket=/tmp/software.sock' > expected; cmp out expected",
        "MariaDB client option-file parsing",
    )
    add(
        "mariadb-dumpslow mysqldumpslow",
        """printf '# Time: 260930 12:00:00\n# User@Host: root[root] @ localhost []\n# Query_time: 1.000000 Lock_time: 0.000000 Rows_sent: 1 Rows_examined: 1\nSET timestamp=1790769600;\nSELECT 42;\n' > slow.log
"$1" slow.log > out; grep -q 'SELECT N' out; grep -q 'Count: 1' out""",
        "parse and aggregate an actual MariaDB slow-query log format",
    )
    add(
        "mysql_tzinfo_to_sql mariadb-tzinfo-to-sql",
        '"$1" /usr/share/zoneinfo/UTC Etc/UTC > out; '
        "grep -q 'INSERT INTO time_zone_name' out; grep -q 'Etc/UTC' out",
        "translate a timezone binary into MariaDB SQL statements",
    )
    for names, fixture, timeout, purpose in (
        (
            "nginx",
            "NginxRuntimeProbe.py",
            150,
            "Nginx master/workers, concurrent HTTP and reload",
        ),
        (
            "redis-server",
            "RedisRuntimeProbe.py",
            150,
            "Redis transactions, Lua, persistence and restart",
        ),
        (
            "redis-cli",
            "RedisCliProbe.py",
            60,
            "Redis CLI transactions and verified RDB snapshot",
        ),
        (
            "ffmpeg",
            "FfmpegRuntimeProbe.py",
            180,
            "FFmpeg encode, decode and content checks",
        ),
        (
            "mariadbd",
            "MariadbRuntimeProbe.py",
            240,
            "MariaDB transaction, concurrency and recovery",
        ),
    ):
        add(names, "python3 " + fixture, purpose, timeout)
        for name in names.split():
            result[name]["fixtures"] = [fixture]
    return result
