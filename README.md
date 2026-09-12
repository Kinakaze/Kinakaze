# Kinakaze

2026-09-16 起继续推进通用工具、模块化和性能改进，当前批次与后续验收目标见 [持续推进记录](docs/progress.md)。

在 Windows x86-64 上运行 Linux x86-64 ELF 程序的独立实现工程。一个活动 native worker 对应一个 Linux Process，Linux 线程运行在所属 worker 内；公共管理进程维护逻辑 PID、进程关系和状态事务。运行不要求管理员权限或自定义驱动。

当前原生模块构建已验证 BusyBox/Bash、Python、curl/wget、Debian 包管理和 SQLite 行为；Node 的 VM、线程、异步文件、子进程环境和 Unix socket/TCP 行为已通过；sshd 已通过真实密钥登录、远程管道及退出码验证。GCC 和 Clang 已实际编译并运行包含线程、TLS、动态库和 fork 的程序；Redis 已通过事务、Lua、Unix socket、RDB/AOF 后台持久化和重启恢复。PostgreSQL 已通过版本启动并推进至 initdb bootstrap，后续初始化仍失败；FFmpeg 尚未通过。Java JIT、线程、文件及默认 ProcessBuilder 已通过；posix_spawn 的文件操作、PATH、信号掩码、失败回收及 fork 后状态也已通过真实程序验证。OpenGL、Minecraft、Docker、CUDA 的历史结果及适用构建见 [验证记录](docs/validation.md)，本轮证据与限制见 [持续推进记录](docs/progress.md)。

持续目标已按最新架构、清理和完整 Docker 要求更新，见 [目标](docs/goal.md)。

## 运行结构

Linux ELF / 第三方 ELF DSO 由 `libs/ld-linux-x86-64` 的实际装载器处理依赖、版本、TLS、重定位和 DWARF 回溯。自研模块直接从 `rootfs/lib/<SONAME>` 加载原生 PE `.so`，使用原生导出地址；公共状态通过 runtime 的 C ABI 或 init RPC 管理。

不使用全局或嵌入式模块清单。Cargo 描述编译依赖，模块自己的 `exports.def` 是标准链接器输入，COFF 导入库给出实际 `.so` 导入名。打包不重写 DLL、导入表或标准库堆函数。Linux 导出别名标记 PRIVATE，只供动态查找，防止进入宿主导入库并替换 Windows CRT 的同名函数。

源代码直接位于 `libs/<模块>/src`，整个项目使用一个 Cargo workspace。runtime 保留 engine 入口和公共 RPC。原生 Unix socket 句柄 keeper 与 usernet broker 经 runtime 的独立 C ABI 启动，通过 init 的 Helper 角色认证并纳入 Job；辅助进程没有 Linux PID 或管理权限。原生 Rust 动态库之间仍有内部 Rust ABI 依赖，完整 C ABI 边界与模块私有状态恢复尚未完成。图形兼容库及 ld/libdl 仍有 PE 导出别名。VEH、AOT 缓存、指令解码与 trampoline 已移至 engine；ld.so 通过 runtime 的借用缓冲区 C ABI 请求执行准备。

模块无需管理状态时没有初始化管理入口。需要时使用固定的版本化 C ABI 获取 runtime 接口。DLL 私有内存由所属模块管理；跨模块输出采用调用方缓冲区或明确的分配/释放接口。fork 需要模块状态序列化、内核句柄转交、DLL 重新装载及恢复调用，不能靠重写所有 DLL/堆来代替状态边界。

## 目录

| 路径 | 职责 |
| --- | --- |
| `apps/init/`、`apps/worker/` | 公共管理服务、原生 worker 与 CLI |
| `crates/abi/`、`protocol/`、`manager/` | C ABI、RPC 和进程/状态事务 |
| `crates/runtime/` | engine 入口与唯一管理 RPC 会话 |
| `crates/loader/` | 模块装载引用、登记与 fork 接续 |
| `crates/bridge/` | 原生导出读取、模块引用、仅构建期使用的 ELF 链接输入 |
| `engine/crates/` | 执行、进程、TLS、VFS 与显式 guest 内存 |
| `libs/ld-linux-x86-64/` | ELF/PE 混合链接、dl*、装载器 fork 状态及 DWARF 展开 |
| `libs/<模块>/` | 模块源码和标准导出定义，无 implementation 子层 |
| `tools/abi/` | 从真实 ELF 记录的版本需求及来源证据，仅用于构建检查 |
| `tools/native-exports/` | 从实际定义更新链接器导出及对象尺寸查询 |
| `tools/packager/` | 按 COFF 导入名原样发布 DLL，验证导入和导出 |
| `tools/prepare-root.py`、`guest-deps/` | Linux 文件树与外部依赖准备 |

## 构建

需要 Rust MSVC 工具链、MSVC 库工具、Python 3.11+ 和 LLVM（`lld-link`、`clang`、`ld.lld`）。

```powershell
./tools/build.ps1 -DistDirectory artifacts/portable-dist
./tools/build.ps1 -Release -DistDirectory artifacts/portable-release
```

`tools/native-link.py` 在链接前合并 rustc 与模块的标准 `.def` 输入；不修改输出二进制。`-RefreshExports` 更新模块导出定义；`-SkipTests` / `-SkipFormat` 仅用于明确选择的构建检查范围。

最终目录只包含：

```text
init.exe
worker.exe
rootfs/
  lib/          # 原字节重命名的原生 SO 及所需 DLL
  usr/lib/      # 第三方 ELF 依赖及程序资源
  bin/ ...      # Linux 标准文件树
```

没有发布 `sdk/`、`host/`、模块清单或报告。测试程序链接使用构建目录中的 `target/<profile>/elf-imports/`。如果 rootfs 安装了 GCC/Clang，准备完 Debian 开发包后执行 `./tools/build.ps1 -Development -DistDirectory artifacts/portable-dist`，在标准 `usr/lib/x86_64-linux-gnu/` 内生成无版本名的 ELF 链接输入。它们提供符号、版本与真实 SO 的 SONAME；生成程序仍直接调用 `rootfs/lib/` 的原生实现，没有运行时中转层。更新原生 ABI 后需重新执行该开发打包步骤。

AOT 缓存按程序需要生成于 `rootfs/var/cache/kinakaze/aot/`。当前验收目录是 `artifacts/portable-dist`；入口已验证在其他工作目录、空 PATH 下启动，原生模块仍依赖 Windows 系统与 VC Runtime。干净 Windows 安装上的独立部署尚待验收。

## 准备客体文件与运行

`prepare-root.py` 从已有 Linux 程序文件树复制 BusyBox、curl 及所选 JRE/Minecraft 文件。`--source` 需要实际 Linux ELF 和相关文件；项目当前联调来源为旧工程的 `E:/Naka/crysoacu/target/debug` 文件树。脚本检查递归 `DT_NEEDED`、解释器及已知 `dlopen` 依赖，只下载 `dependencies.lock.json` 中已登记并校验哈希的缺失依赖。`--offline` 只用本地校验缓存。

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --java

./target/debug/worker.exe run --root artifacts/guest-root --dist dist -- /usr/bin/busybox echo hello
./target/debug/worker.exe run --root artifacts/guest-root --dist dist -- /bin/sh -c 'printf "b\na\n" | /bin/busybox sort'
./target/debug/worker.exe run --root artifacts/guest-root --dist dist -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version
```

发布目录内的 `worker.exe run -- /bin/sh` 可直接启动：dist 使用 EXE 所在目录，root 使用相邻 `rootfs/`。`--root`、`--dist` 可显式指定开发测试目录；程序路径仍为绝对 Linux 路径。`--cwd /linux/path` 指定客体工作目录，`--` 之后全部作为客体 argv。每次 `run` 创建所属管理会话和 worker，向调用者返回逻辑进程退出状态。若使用其他 JRE 目录，同时调整准备脚本的 `--java-home` 和运行命令。

### init 运行状态页面

可选 `--web 127.0.0.1:PORT` 启用当前会话的只读 WebUI，端口 `0` 自动选择空闲端口，地址打印到 stderr：

```powershell
./target/debug/worker.exe run --root artifacts/guest-root --dist dist --web 127.0.0.1:0 -- /bin/sh
```

页面显示 Linux PID、父 PID、执行代次、宿主 PID、状态、宿主工作集、私有内存与累计 CPU 时间。默认不开启 WebUI；只有请求 `/api/state` 时采集，页面可见时每 2 秒刷新，隐藏时取消请求并暂停。不使用就不采集，不维护后台指标缓存。会话结束后 WebUI 一同退出。也可向 init 直接传入 `--web`；已移除环境变量开关，避免继承宿主环境意外启用采集。

### 常用工具验收

`prepare-root.py --program 相对路径` 可重复添加程序与 ELF 依赖闭包；`--tree 相对目录` 添加标准库或编译器资源；`--busybox-applet gzip` 等参数显式添加 `/bin` 命令入口。

`--package 名称` 从锁定 SHA-256 的 Debian 包加入程序、标准库和依赖。例如 `--package netbase --package sqlite3 --package python3.11-minimal --package libpython3.11-stdlib` 可准备 SQLite 和 Python 文件；包选定的库优先于源目录的旧副本。SQLite 的内存查询、磁盘 WAL/事务/完整性检查，以及 Python 的线程、asyncio、Unix socket、归档压缩、数据库、子进程和 HTTPS 验证均已通过。Node 完整行为探针和 SSH 密钥登录已通过；nginx 已验证双 worker、HTTP 文件/Range/并发、重载和优雅退出；Make/Ninja 增量编译、pkgconf、Git 基础仓库、sed/grep/find/patch、jq、OpenSSL、file 和 ps/free 已通过行为测试。XInput/RandR/Xext 已拆为独立原生 SO，XImage、共享图像与音频参数已有客体行为探针；rsync 已验证预分配、硬/软链接、校验更新、删除和 TCP daemon 的认证/压缩往返。Xdbe 双缓冲和 X11 事件转换已有像素、回调与 fork 行为探针。整组图形/音频接口尚未完成，FFmpeg 当前仍缺 SDL2 所需 Pulse 主循环，Git 传输也尚未通过。OpenSSL 默认配置已补齐；新增 nginx TLS/Unix 上游探针仍被证书生成异常阻塞，尚未验收。可复现命令见 [软件包准备说明](tools/guest-deps/README.md)。

```powershell
python tests/guest/tool-matrix.py --root artifacts/guest-root --dist dist --report artifacts/tool-matrix.json
```

Docker 采用独立的 `tests/guest/docker-matrix.json`：核心程序启动、runc OCI 配置生成、iptables/ip6tables 启动，以及默认 dockerd 的 Unix socket API、关闭与目录清理共 10 项检查已通过。输入显式使用锁定的 `netbase`、`iptables` 包。真实生命周期探针也已通过本地镜像导入、容器启动、PID 1/proc、绑定卷、桥接 IP、exec、退出码 23、再次启动、删除容器/镜像及 daemon 关闭后目录清理。最新独立探针已验证容器内 IPv4 HTTP，跨容器 IP 连接仍返回 EIO，DNS 尚未达到；IPv6 通配监听接收 IPv4 的双栈路径也未通过。日志仍有 shim/runc 清理、cgroup 和网络告警，完整 Docker 能力尚未验收。准备与复现命令见 [持续推进记录](docs/progress.md)。

基础 libc 接口也有独立真实 ELF 回归：

```powershell
python tests/guest/run-libc-tools.py
python tests/guest/run-network-db.py
python tests/guest/run-positioned-io.py
python tests/guest/tool-matrix.py --root artifacts/tool-root --only python --only python-runtime
python tests/guest/curl-smoke.py --worker target/debug/worker.exe --root artifacts/tool-root --dist dist --python --report artifacts/python-curl-tls-report.json
python tests/guest/tool-matrix.py --root artifacts/tool-root --only bash --only dpkg-deb-roundtrip --only dpkg-install-remove
```

包测试使用临时目录与独立 dpkg 数据库，校验包构造、解包、安装和移除。需要按推进记录准备 tar、压缩工具、dpkg 辅助程序及架构数据表。

矩阵逐项执行有超时限制的真实 Linux 程序，保留日志和二进制哈希；区分功能探针、仅启动探针、缺少输入和运行失败。存在未通过项时返回非零，不将缺少工具计为成功。详细准备命令与当前缺口见 [持续推进记录](docs/progress.md)。

### CUDA Driver API

Linux CUDA Driver API 已经通过系统 NVIDIA 驱动执行真实 GPU 计算：设备/上下文、显存与 pinned host memory、异步传输、stream/event、PTX JIT 和 cubin 加载。函数指针查询返回经过 ABI 转换的入口。provider 默认不加载驱动，不使用就不查询设备或采集指标。

```powershell
python tests/guest/run-cuda.py --fork
python tools/cuda/prepare-sdk.py --compiler
python tools/cuda/verify-abi.py
python tests/guest/run-cuda.py --ptxas artifacts/cuda-sdk/ptxas.exe --gpu-architecture sm_89
```

`sm_89` 是本次 RTX 4060 的目标；其他设备需要匹配的架构。完整 Linux CUDA Runtime、PyTorch 和 CUDA 数值库仍待验证。具体接口、资源所有权和限制见 [CUDA provider](libs/libcuda/README.md)。

Minecraft 的目标是 **Java 版客户端**，包括窗口、渲染、输入、音频和游戏生命周期。启动工具根据已有官方版本元数据选择 Linux 依赖并校验 JAR 哈希，使用独立 `minecraft/v2-demo` 游戏目录和 demo 参数：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --minecraft-version 26.2
python tools/run-minecraft.py --root artifacts/guest-root --dist dist --version 26.2 --print-command
python tools/run-minecraft.py --root artifacts/guest-root --dist dist --version 26.2
```

准备和启动工具已经存在；这组命令目前用于继续联调，不能据此认定客户端已运行成功。旧存档不在准备脚本复制范围，启动使用独立 demo 目录。客户端窗口、场景、输入、音频和正常退出的验收条件见 [验证记录](docs/validation.md)。

## 公共状态与实现边界

管理对象按 Process 和模块身份寻址，RPC 不传宿主指针或 HANDLE。fork 的 prepare/adopt/ready/commit/abort 事务与 Copy/Share/Reset 状态策略保留；真实 ELF fork 进一步参与内存、寄存器、TLS、FD 和装载状态接续。exec 可更换承载映像的 native worker，但保留逻辑 PID。进程死亡以匹配 native PID 和创建时间的进程 HANDLE 为依据，连接 EOF 不直接等价于进程退出。

基础认证、Job 回收和 worker 分离已经实现；完整容器隔离、跨 worker 共享地址空间的全部 clone 组合、异步 signal、完整展开与 DSO 卸载、AIO、录音及 ALSA mmap 等仍需补齐和验证。不能把旧实现的存在、导出清单或启动成功代替这些 ABI 的完整性证明。

全局方案和逐 syscall 目标继续参照 [整体设计](docs/architecture-v2.md)、[syscall 清单](docs/architecture-v2-syscalls.csv)；当前工程状态以 [ABI 实施路线](docs/abi-roadmap.md) 和 [验证记录](docs/validation.md) 为准。
