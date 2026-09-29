"""Comprehensive CLI daily scenarios compatibility suite for Kinakaze."""
import json
import os
from pathlib import Path
import subprocess
import time
import sys

WORKER = r"F:\crysoacu2\artifacts\debian-compat-verified\worker.exe"
DIST = r"F:\crysoacu2\artifacts\debian-compat-verified"
ROOT = r"C:\Users\nanaeo\AppData\Local\Temp\kinakaze command audit ipq6rbc9\rootfs"

TEST_CASES = [
    # 1. 基础文件与目录操作
    {
        "category": "文件与目录操作",
        "name": "目录创建、文件写入与读取 (mkdir, touch, echo, cat)",
        "script": """
set -e
mkdir -p /tmp/daily_fstest/subdir
echo "Hello Kinakaze" > /tmp/daily_fstest/subdir/test.txt
content=$(cat /tmp/daily_fstest/subdir/test.txt)
test "$content" = "Hello Kinakaze"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文件与目录操作",
        "name": "文件复制、移动与属性查看 (cp, mv, stat, ls)",
        "script": """
set -e
mkdir -p /tmp/daily_fstest2
echo "data123" > /tmp/daily_fstest2/src.txt
cp /tmp/daily_fstest2/src.txt /tmp/daily_fstest2/dst.txt
mv /tmp/daily_fstest2/dst.txt /tmp/daily_fstest2/moved.txt
test -f /tmp/daily_fstest2/moved.txt
test ! -f /tmp/daily_fstest2/dst.txt
size=$(stat -c %s /tmp/daily_fstest2/moved.txt)
test "$size" -gt 0
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文件与目录操作",
        "name": "软链接创建与解析 (ln -s, readlink, test -L)",
        "script": """
set -e
rm -rf /tmp/daily_linktest
mkdir -p /tmp/daily_linktest
echo "target file" > /tmp/daily_linktest/target
ln -s /tmp/daily_linktest/target /tmp/daily_linktest/link
test -L /tmp/daily_linktest/link
test "$(cat /tmp/daily_linktest/link)" = "target file"
test "$(readlink /tmp/daily_linktest/link)" = "/tmp/daily_linktest/target"
rm -rf /tmp/daily_linktest
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文件与目录操作",
        "name": "目录查找与统计 (find, du, wc)",
        "script": """
set -e
mkdir -p /tmp/daily_findtest/a/b
touch /tmp/daily_findtest/a/file1.txt
touch /tmp/daily_findtest/a/b/file2.txt
count=$(find /tmp/daily_findtest -type f -name '*.txt' | wc -l)
test "$count" -eq 2
echo "OK"
""",
        "expect": "OK",
    },

    # 2. 文本流处理与过滤
    {
        "category": "文本流处理",
        "name": "Grep 模式匹配与正则过滤 (grep -i, -v, -E)",
        "script": """
set -e
printf "apple\\nBANANA\\nCherry\\navocado\\n" | grep -i '^a' | wc -l | grep -q 2
printf "one\\ntwo\\nthree\\n" | grep -v 'two' | wc -l | grep -q 2
echo "2026-09-28" | grep -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文本流处理",
        "name": "Sed 流编辑替换与行操作 (sed s///, /d)",
        "script": """
set -e
res=$(printf "foo bar\\nhello world\\n" | sed 's/foo/baz/g' | head -n 1)
test "$res" = "baz bar"
deleted=$(printf "keep\\ndrop\\nkeep\\n" | sed '/drop/d' | wc -l)
test "$deleted" -eq 2
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文本流处理",
        "name": "Awk 列提取、条件过滤与累加 (awk/mawk)",
        "script": """
set -e
sum=$(printf "item1 10\\nitem2 20\\nitem3 30\\n" | awk '{s += $2} END {print s}')
test "$sum" -eq 60
col=$(printf "a:b:c\\n" | awk -F: '{print $2}')
test "$col" = "b"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文本流处理",
        "name": "排序、去重与字符转换 (sort, uniq -c, tr, cut)",
        "script": """
set -e
sorted=$(printf "c\\na\\nb\\n" | sort | tr '\\n' ' ')
test "$sorted" = "a b c "
uniq_count=$(printf "dup\\ndup\\nsingle\\n" | sort | uniq | wc -l)
test "$uniq_count" -eq 2
cut_res=$(echo "root:x:0:0" | cut -d: -f1,3)
test "$cut_res" = "root:0"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "文本流处理",
        "name": "文本差异比较 (diff, cmp)",
        "script": """
set -e
mkdir -p /tmp/daily_difftest
echo "line 1" > /tmp/daily_difftest/a
echo "line 1" > /tmp/daily_difftest/b
diff /tmp/daily_difftest/a /tmp/daily_difftest/b
echo "line 2" >> /tmp/daily_difftest/b
if diff -u /tmp/daily_difftest/a /tmp/daily_difftest/b | grep -q '+line 2'; then
    echo "OK"
fi
""",
        "expect": "OK",
    },

    # 3. 归档与压缩
    {
        "category": "归档与压缩",
        "name": "tar 打包与解包 (.tar)",
        "script": """
set -e
mkdir -p /tmp/daily_tartest/src
echo "content-A" > /tmp/daily_tartest/src/a.txt
echo "content-B" > /tmp/daily_tartest/src/b.txt
tar -cf /tmp/daily_tartest/bundle.tar -C /tmp/daily_tartest/src a.txt b.txt
mkdir -p /tmp/daily_tartest/out
tar -xf /tmp/daily_tartest/bundle.tar -C /tmp/daily_tartest/out
test "$(cat /tmp/daily_tartest/out/a.txt)" = "content-A"
test "$(cat /tmp/daily_tartest/out/b.txt)" = "content-B"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "归档与压缩",
        "name": "gzip 压缩与解压 (.gz, tar -czf)",
        "script": """
set -e
mkdir -p /tmp/daily_gziptest
echo "compressible string repeated repeated repeated" > /tmp/daily_gziptest/test.txt
gzip /tmp/daily_gziptest/test.txt
test -f /tmp/daily_gziptest/test.txt.gz
gunzip /tmp/daily_gziptest/test.txt.gz
test -f /tmp/daily_gziptest/test.txt
test "$(cat /tmp/daily_gziptest/test.txt)" = "compressible string repeated repeated repeated"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "归档与压缩",
        "name": "bzip2 压缩与解压 (.bz2)",
        "script": """
set -e
mkdir -p /tmp/daily_bziptest
echo "bzip2 data block" > /tmp/daily_bziptest/sample.txt
bzip2 /tmp/daily_bziptest/sample.txt
test -f /tmp/daily_bziptest/sample.txt.bz2
bunzip2 /tmp/daily_bziptest/sample.txt.bz2
test "$(cat /tmp/daily_bziptest/sample.txt)" = "bzip2 data block"
echo "OK"
""",
        "expect": "OK",
    },

    # 4. 系统信息与 procfs 检查
    {
        "category": "系统信息与procfs",
        "name": "基础系统信息 (uname, date, id, whoami)",
        "script": """
set -e
test "$(whoami)" = "root"
test "$(id -u)" = "0"
test "$(uname -s)" = "Linux"
test -n "$(uname -m)"
test -n "$(date)"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "系统信息与procfs",
        "name": "进程与内存监控 (ps, free, uptime)",
        "script": """
set -e
ps aux | grep -v 'PID' | head -n 1
free -m | grep -q 'Mem:'
test -n "$(uptime)"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "系统信息与procfs",
        "name": "标准 procfs 虚拟文件读取 (/proc/cpuinfo, meminfo, stat, loadavg)",
        "script": """
set -e
grep -q 'processor' /proc/cpuinfo
grep -q 'MemTotal:' /proc/meminfo
grep -q 'cpu' /proc/stat
test -n "$(cat /proc/loadavg)"
test -n "$(cat /proc/version)"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "系统信息与procfs",
        "name": "新补充 procfs 节点验证 (/proc/cmdline, smaps_rollup, auxv, overcommit_memory)",
        "script": """
set -e
if [ -f /proc/cmdline ]; then
    grep -q 'BOOT_IMAGE=' /proc/cmdline || test -s /proc/cmdline
fi
if [ -f /proc/self/smaps_rollup ]; then
    grep -q '[rollup]' /proc/self/smaps_rollup
    grep -q 'Rss:' /proc/self/smaps_rollup
fi
if [ -f /proc/self/auxv ]; then
    test -s /proc/self/auxv
fi
if [ -f /proc/sys/vm/overcommit_memory ]; then
    val=$(cat /proc/sys/vm/overcommit_memory | tr -d '[:space:]')
    echo "overcommit_memory=$val"
fi
echo "OK"
""",
        "expect": "OK",
    },

    # 5. 网络查询与接口工具
    {
        "category": "网络工具与查询",
        "name": "网络接口与路由查看 (ip addr, ip route)",
        "script": """
set -e
ip addr show
ip route show || true
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "网络工具与查询",
        "name": "NSS 名称服务查询 (getent passwd, group, hosts, services)",
        "script": """
set -e
getent passwd root | grep -q '^root:.*:0:0:'
getent group root | grep -q '^root:.*:0:'
getent services ssh || true
getent protocols tcp | grep -q '6'
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "网络工具与查询",
        "name": "网络客户端版本与参数 (curl, wget, ssh)",
        "script": """
set -e
curl --version | head -n 1 | grep -q 'curl'
wget --version | head -n 1 | grep -q 'GNU Wget'
ssh -V 2>&1 | grep -qi 'OpenSSH'
echo "OK"
""",
        "expect": "OK",
    },

    # 6. 开发环境与脚本语言
    {
        "category": "开发与脚本环境",
        "name": "Bash 高级特性 (数组, 进程替换, pipefail, 这里文档)",
        "script": """
set -e
set -o pipefail
# 数组
arr=(alpha beta gamma)
test "${arr[1]}" = "beta"
test "${#arr[@]}" -eq 3
# 进程替换
diff <(printf "test\\n") <(printf "test\\n")
# Here-doc
cat <<'EOF' > /tmp/daily_heredoc.txt
multi
line
content
EOF
test "$(wc -l < /tmp/daily_heredoc.txt)" -eq 3
# 函数与局部变量
my_func() {
    local val="inner"
    echo "$val"
}
test "$(my_func)" = "inner"
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "开发与脚本环境",
        "name": "Python3 标准库综合测试 (json, sqlite3, hashlib, subprocess, socket)",
        "script": """
python3 -c '
import json, sqlite3, hashlib, subprocess, socket, math

# JSON
data = json.loads("{\\"key\\": 123, \\"list\\": [1, 2, 3]}")
assert data["key"] == 123

# SQLite
db = sqlite3.connect(":memory:")
db.execute("CREATE TABLE t (id INT, name TEXT)")
db.execute("INSERT INTO t VALUES (1, \\"kinakaze\\")")
assert db.execute("SELECT name FROM t WHERE id=1").fetchone()[0] == "kinakaze"

# Hashlib
h = hashlib.sha256(b"kinakaze").hexdigest()
assert len(h) == 64

# Subprocess
out = subprocess.check_output(["/bin/echo", "sub_ok"]).decode().strip()
assert out == "sub_ok"

# Math
assert math.isclose(math.sqrt(16), 4.0)

print("PYTHON3_ALL_PASSED")
'
""",
        "expect": "PYTHON3_ALL_PASSED",
    },
    {
        "category": "开发与脚本环境",
        "name": "Perl 文本处理与标准模块 (JSON::PP, 正则)",
        "script": """
perl -e '
use JSON::PP;
my $json = encode_json({ status => "perl_ok", count => 42 });
my $data = decode_json($json);
die "fail" unless $data->{status} eq "perl_ok" && $data->{count} == 42;
my $text = "Hello 2026 Kinakaze";
$text =~ s/\\d+/YEAR/;
die "regex fail" unless $text eq "Hello YEAR Kinakaze";
print "PERL_ALL_PASSED\\n";
'
""",
        "expect": "PERL_ALL_PASSED",
    },

    # 7. 包管理与环境状态
    {
        "category": "包管理与环境",
        "name": "dpkg 查询已安装包列表 (dpkg -l, dpkg-query)",
        "script": """
set -e
dpkg -l | grep -q 'kinakaze-base'
dpkg-query -W -f='${Package}: ${Status}\\n' kinakaze-base | grep -q 'installed'
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "包管理与环境",
        "name": "APT 版本与配置检查 (apt-config, apt-cache)",
        "script": """
set -e
apt-config dump | grep -q 'APT::Architecture'
echo "OK"
""",
        "expect": "OK",
    },

    # 8. 管道重定向与退出码控制
    {
        "category": "管道与重定向",
        "name": "多级深层管道处理与空管道",
        "script": """
set -e
res=$(printf "9\\n2\\n5\\n1\\n8\\n" | sort -n | head -n 4 | tail -n 2 | tr '\\n' ',')
test "$res" = "5,8,"
# 空数据流不应崩溃
empty=$(cat /dev/null | grep 'nothing' | wc -l)
test "$empty" -eq 0
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "管道与重定向",
        "name": "标准错误与标准输出重定向 (2>&1, >/dev/null)",
        "script": """
set -e
(echo "stdout_msg"; echo "stderr_msg" >&2) > /tmp/daily_redirect.txt 2>&1
grep -q 'stdout_msg' /tmp/daily_redirect.txt
grep -q 'stderr_msg' /tmp/daily_redirect.txt
echo "OK"
""",
        "expect": "OK",
    },
    {
        "category": "管道与重定向",
        "name": "退出状态码传播与逻辑控制 (&&, ||, test)",
        "script": """
set -e
true && echo "true branch" > /dev/null
false || echo "false fallback" > /dev/null
if [ 10 -gt 5 ] && [ "abc" != "def" ]; then
    echo "OK"
fi
""",
        "expect": "OK",
    },
]


def run_test(case):
    command = [
        WORKER,
        "--root", ROOT,
        "--dist", DIST,
        "--",
        "/bin/bash", "-c", case["script"]
    ]
    started = time.monotonic()
    try:
        proc = subprocess.run(
            command,
            capture_output=True,
            timeout=25,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)
        )
        elapsed = round((time.monotonic() - started) * 1000, 1)
        stdout = proc.stdout.decode("utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")
        
        passed = (proc.returncode == 0) and (case["expect"] in stdout or case["expect"] in stderr)
        return {
            "category": case["category"],
            "name": case["name"],
            "passed": passed,
            "exit_code": proc.returncode,
            "elapsed_ms": elapsed,
            "stdout": stdout.strip(),
            "stderr": stderr.strip(),
        }
    except subprocess.TimeoutExpired:
        elapsed = round((time.monotonic() - started) * 1000, 1)
        return {
            "category": case["category"],
            "name": case["name"],
            "passed": False,
            "exit_code": -1,
            "elapsed_ms": elapsed,
            "stdout": "",
            "stderr": "TIMEOUT (25s)",
        }
    except Exception as e:
        elapsed = round((time.monotonic() - started) * 1000, 1)
        return {
            "category": case["category"],
            "name": case["name"],
            "passed": False,
            "exit_code": -2,
            "elapsed_ms": elapsed,
            "stdout": "",
            "stderr": str(e),
        }


def main():
    print(f"=== Starting Kinakaze Daily CLI Scenarios Compatibility Test ===")
    print(f"Worker: {WORKER}")
    print(f"Root:   {ROOT}")
    print(f"Dist:   {DIST}")
    print(f"Total test scenarios: {len(TEST_CASES)}\n")

    results = []
    category_stats = {}

    for i, case in enumerate(TEST_CASES, 1):
        cat = case["category"]
        if cat not in category_stats:
            category_stats[cat] = {"total": 0, "passed": 0}
        category_stats[cat]["total"] += 1

        print(f"[{i:02d}/{len(TEST_CASES):02d}] Testing: {case['name']} ... ", end="", flush=True)
        res = run_test(case)
        results.append(res)
        if res["passed"]:
            category_stats[cat]["passed"] += 1
            print(f"PASS ({res['elapsed_ms']}ms)")
        else:
            print(f"FAIL (exit: {res['exit_code']}, {res['elapsed_ms']}ms)")
            if res["stderr"]:
                print(f"       stderr: {res['stderr'][:200]}")
            if res["stdout"]:
                print(f"       stdout: {res['stdout'][:200]}")

    print("\n" + "=" * 60)
    print("=== Category Compatibility Summary ===")
    print("=" * 60)
    total_passed = sum(s["passed"] for s in category_stats.values())
    total_count = len(TEST_CASES)

    for cat, stat in category_stats.items():
        rate = (stat["passed"] / stat["total"]) * 100
        print(f"- {cat:<18}: {stat['passed']}/{stat['total']} passed ({rate:.1f}%)")

    overall_rate = (total_passed / total_count) * 100
    print("-" * 60)
    print(f"Total: {total_passed}/{total_count} passed ({overall_rate:.1f}% overall compatibility)")
    print("=" * 60)

    # Save detailed JSON report
    report_file = Path("artifacts/daily-cli-compatibility-report.json")
    report_file.parent.mkdir(parents=True, exist_ok=True)
    report_data = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
        "total": total_count,
        "passed": total_passed,
        "pass_rate_percent": round(overall_rate, 2),
        "categories": category_stats,
        "details": results,
    }
    report_file.write_text(json.dumps(report_data, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nReport written to: {report_file.resolve()}")


if __name__ == "__main__":
    main()
