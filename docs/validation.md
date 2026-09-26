# 集成验证记录

2026-09-26 v0.1.0：[正式版本运行与验证](runtime-release-v0.1.0.md)。完整 release 构建通过：1,853 项 Rust 测试通过、0 失败、27 项现有忽略；43 项 Python 测试通过。完整基础 rootfs 的空目录安装、外部清单优先、并发初始化、真实 PPID 和重连启动客户端等 8 项运行验收通过，运行 ZIP 解压后复验通过。该版本不打包 Java/Minecraft 或编译开发环境；下列应用结果保留为历史记录。

2026-09-23 futex：[MAP_SHARED 跨进程实现](futex-shared-2026-09-23.md)。普通 WAIT/WAKE、BITSET、REQUEUE/CMP_REQUEUE 和 WAKE_OP 使用内核域共享队列；最终发布包的 5 项客体探针、8 项单元测试通过，包含真实 fork/exec、文件别名、信号和异常退出恢复。PI/robust-list 不在此次实现范围内。

2026-09-23 Minecraft：[白屏与后续音频初始化崩溃修复](minecraft-rendering-2026-09-23.md)。发布包 `artifacts/minecraft-render-dist` 已显示真实主菜单，Continue 按钮响应正常，窗口关闭后退出码为 0；4 项客体探针及 7 项 futex 单元测试通过，世界内游玩尚未验收。

2026-09-23 第六轮：[Java 子进程与跨视图内存修复](startup-performance-round6-2026-09-23.md)。当前源码的发布包已构建并通过通用池、spawn、跨视图探针；Minecraft 26.2 在激活后 18.672 秒提交首帧，仍未达到 200ms，也未将首帧等同完整菜单就绪。

2026-09-22 第二轮：[继续优化大型应用启动](startup-performance-round2-2026-09-22.md)。相同兼容性代码对照中，Java/Firefox 无 AOT 缓存启动耗时进一步减少 44.8%/44.3%；相关核心测试 117 项通过，缓存计划保持一致。Java 整体热启动尚未证明进一步改善。

2026-09-22 新增：[大型应用启动性能实测](startup-performance-2026-09-22.md)。固定源码对照中，Java 版本启动耗时减少 20.3%，Firefox/Chrome 版本启动约快 7.18/5.83 倍；包含真实行为回归、缓存失效验证及未通过的全仓测试边界。

2026-09-22 新增：[软件运行边界实测](software-boundaries-2026-09-22.md)。固定 v79 的通用软件 48/49 通过，但 SQLite 跨进程锁/WAL、浏览器装载和中文字体显示仍有明确失败；以下历史结果不可外推到这些场景。

浏览器后续修复与独立验收：[Chrome / Firefox 启动推进](browser-startup-2026-09-22.md)。

记录日期：2026-09-12。环境为 Windows x86-64、Rust/Cargo 1.95.0、MSVC target。本文区分实际运行结果、模块测试和仍待验收的场景。目录中的导出数量以及应用依赖闭包检查都不等于应用已经运行成功。

## 当前应用结果

| 场景 | 实际结果 | 证明范围与边界 |
| --- | --- | --- |
| Linux BusyBox | shell、管道、重定向、文件操作返回 0 | 通过真实 ELF 入口运行；尚未对所有 BusyBox applet 和选项做完整矩阵验证 |
| Linux 进程 ABI 探针 | 输出 `PROCESS_ABI_OK`，返回 0 | `fork` 返回值与父子 PID、子退出状态、`execve` 保持逻辑 PID、`waitpid`、`posix_spawn` 文件操作；还需扩大 clone、并发与失败恢复覆盖 |
| Linux Java 25 `-version` | 输出 Temurin 25.0.4.1+1、64-Bit Server VM，返回 0 | 实际 Linux JRE/JVM 装载和启动；不是调用 Windows Java |
| `JavaRuntimeProbe` | 输出 `JAVA_JIT_THREADS_FILES_OK`，返回 0 | 默认 JIT 模式下的循环数值校验、8 个异步任务、UTF-8 文件写读/真实路径/删除；不代表所有 JVM/JNI 行为已覆盖 |
| 跨 worker 内存访问 | 输出 `PROCESS_MEMORY_OK`，返回 0 | 父进程读取/修改子进程独立内存，死亡 PID 返回 ESRCH、原始 syscall 310、自身错误地址 EFAULT；跨进程权限模型仍待扩展 |
| Java `ProcessBuilder` | 输出 `JAVA_SPAWN_OK`，返回 0 | 默认派生机制，连续 8 次检查 stdout/stderr 合流和退出码 7；没有强制 FORK 或关闭性能数据 |
| Linux curl | 9 项实际请求验证全部通过 | 域名解析、HTTP 二进制 GET/POST、重定向、404 返回 22、可信 CA TLS、错误 CA 和主机名不符返回 60 |
| LWJGL OpenGL | `LWJGL_SMOKE_OK`，返回 0 | GLFW 窗口、shader、三角形渲染，NVIDIA RTX 4060 / OpenGL 3.2；未单独断言像素结果 |
| Minecraft Java 版客户端 | 修复包已显示主菜单、响应按钮并正常退出 | `minecraft-render-dist` 实际画面及 WM_CLOSE 后退出码 0 已验证；世界内游玩尚未验收，未达到 200ms |

补充日志：`artifacts/latest-probe-0.stdout.log` 为 `PROCESS_MEMORY_OK`；`artifacts/latest-probe-1.stdout.log` 同时含 `JAVA_JIT_THREADS_FILES_OK` 和 `JAVA_SPAWN_OK`，两者退出 0、stderr 为空。

本轮本地证据包括 `artifacts/process-probe.stdout.log` 中的 `PROCESS_ABI_OK`、`artifacts/java-runtime.stdout.log` 中的 `JAVA_JIT_THREADS_FILES_OK` 和 `artifacts/java-version.stderr.log` 中的实际 JVM 版本。`artifacts/` 为忽略的本机构建/运行产物；新检出需要按下列步骤重新生成。更改源码或目录后应重新构建并运行对应验收，不能沿用旧产物的通过状态。

最终 `./tools/build.ps1` 已完整通过：89 项 Rust 测试通过、2 项忽略，15 项 catalog 测试通过，管理 smoke 退出后 processes/objects/transactions 均为 0。最终 dist 的 curl 9 项请求、Java `spawn` 和跨 worker 内存探针再次通过，日志为 `artifacts/final-build.log`、`final-curl-report.json`、`final-java.stdout.log`、`final-process-memory.stdout.log`。图形探针输出见 `artifacts/final-lwjgl.stdout.log`。

## 构建与基础检查

从 `F:\crysoacu2` 仓库根目录运行：

```powershell
./tools/build.ps1
# 另一个构建配置，仍须实际执行才算通过
./tools/build.ps1 -Release
```

当前构建使用统一 workspace、原生 `.so` 和标准导出表。SDK ELF 仅用于链接。历史配对清单/rlib 架构的结果保留为基线，本轮结果另见 progress.md。

需要独立重现检查时：

```powershell
python tools/native-exports/generate.py --check
python -m unittest discover -s tools/native-exports
cargo test --workspace --locked --features kinakaze-v2-runtime/guest-engine
cargo test --manifest-path engine/Cargo.toml --workspace --lib --locked
cargo clippy --workspace --all-targets --locked --features kinakaze-v2-runtime/guest-engine -- -D warnings
llvm-readobj --file-headers --program-headers --dynamic-table --dyn-symbols --version-info dist/guest/libc.so.6
```

独立 engine 测试不包含在外层 workspace 的测试选择中，故另列命令。此处提供完整检查入口，不宣称本次所有命令、所有配置已一次性全部通过。已经实际运行的专项验证包括：

- bridge/packager：规范 ELF、GNU 版本、16K 导出上限、数据地址/别名一致性、转发链、序号、BSS、越界与循环拒绝；真实 Windows 数据导出确认没有独立可变副本，LLVM 能读取生成的 ELF。
- core：ELF 解析、TLS、pthread、线程退出、link map/dlinfo、rand48 和 timed signal wait 的专项测试。
- 图形/音频：provider rlib 测试、Pulse/PipeWire 锁及回调并发测试；两个真实 waveOut/WASAPI 端点测试使用静音 PCM，通过不能证明实际可听输出或录音功能。
- resolver/libm/librt：DNS 报文和结构布局、数学与浮点环境、定时器注册/回调/销毁及 SI_TIMER 相关测试。

最初仅三组 provider 的控制链路曾完成 Debug/Release、49 项测试和 Clippy。那是早期基础版本的历史结果，不是当前完整 engine 和应用覆盖率。

## 准备可复现的客体文件树

当前联调使用旧工程 `E:/Naka/crysoacu/target/debug` 中的实际 Linux 程序和 JRE。工具不会生成替代 BusyBox/curl 的 Windows 程序。输入和目标目录必须分离：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --java --check-only
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --java
```

`--check-only` 解析依赖闭包并验证/缓存已登记依赖，不复制目标 root；它不是执行测试。准备完成后的 `kinakaze-artifacts.json` 记录文件哈希、依赖边、来源包和依赖锁哈希。外部包由 `tools/guest-deps/dependencies.lock.json` 固定；缓存可通过 `--offline` 使用。更换 JRE 时须同时传入相应 `--java-home`。

## BusyBox 与进程 ABI

```powershell
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/bin/busybox echo hello
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /bin/sh -c 'printf "b\na\n" | /bin/busybox sort > /tmp/kinakaze-busybox.txt; /bin/busybox cat /tmp/kinakaze-busybox.txt; /bin/busybox rm /tmp/kinakaze-busybox.txt'
```

预期排序内容为 `a`、`b` 两行，命令返回 0。扩展验收还需检查每个 applet 的输出、错误条件和副作用，单个 shell 的最终退出状态不能代替全部中间操作断言。

进程探针源码为 [process_probe.c](../engine/tests/guest/process_probe.c)，使用独立小型 root，方便子进程按相同 Linux 路径再次 exec：

```powershell
./engine/tests/guest/build-process-probe.ps1 -Distribution ./dist -OutputDirectory ./artifacts/process-root
./target/debug/kinakaze-worker.exe run --root artifacts/process-root --dist dist -- /process_probe
```

预期 `PROCESS_ABI_OK` 和退出码 0。探针检查父子逻辑 PID、退出状态 23/31/7、exec 后 PID 不变、spawn 的 stdout 管道、缺失文件返回 ENOENT 且不改变 PID 输出、非法 dup2 返回 EBADF。它不覆盖全部 `clone` 标志、进程共享同步、多线程 fork 和 namespace reaper。

管理 smoke 仍保留独立的 Copy/Share/Reset 与回收检查：

```powershell
./target/debug/kinakaze-worker.exe smoke --dist dist
```

该 smoke 的标量状态测试与上述真实 guest 内存/上下文 fork 探针互为补充；不要将 smoke 中的范围说明理解成整个 engine 仍未实现 fork。

## Java

实际验证用 Linux JRE 位于 `/usr/lib/jvm/jdk-25.0.4.1+1-jre`。先使用可用的宿主 JDK 编译测试类；`javac` 的输出版本须不高于客体 JVM 支持的版本：

```powershell
New-Item -ItemType Directory -Path artifacts/guest-root/tests/java -Force | Out-Null
javac --release 17 -d artifacts/guest-root/tests/java tests/guest/JavaRuntimeProbe.java
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -cp /tests/java JavaRuntimeProbe
```

基础探针要求 `os.name=Linux`，重复验证数值结果，等待 8 个异步任务，并检查中文 UTF-8 文件、真实路径和清理，成功标记为 `JAVA_JIT_THREADS_FILES_OK`。

下面的进程派生验收已经通过：

```powershell
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -cp /tests/java JavaRuntimeProbe spawn
```

`spawn` 分支连续派生 8 个 `/bin/sh`，合并 stdout/stderr，校验内容 `stdoutstderr` 和退出码 7，最后必须出现 `JAVA_SPAWN_OK`。JVM 进程派生涉及多线程 fork、FD 接续和异步清理，应单独回归。

## curl 与 Minecraft 的验收入口

curl 测试需要 Python 的 `cryptography` 包。脚本启动本机临时 HTTP/TLS 服务，通过实际 Linux curl 请求，不访问生产端点：

```powershell
python tests/guest/curl-smoke.py --worker target/debug/kinakaze-worker.exe --root artifacts/guest-root --dist dist --report artifacts/curl-smoke.json
```

验收包括版本/TLS 支持、二进制 GET/POST、重定向、404 错误，以及受信 CA 成功、未知 CA 拒绝、主机名不匹配拒绝。本轮 9 项断言全部通过，报告为 `artifacts/curl-smoke-report.json`。

Minecraft 使用已有版本元数据、客户端 JAR、libraries 和 assets，准备时同时扫描 native JAR 中 Linux x86-64 ELF 的依赖：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --minecraft-version 26.2 --check-only
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --minecraft-version 26.2
python tools/run-minecraft.py --root artifacts/guest-root --dist dist --version 26.2 --print-command
python tools/run-minecraft.py --root artifacts/guest-root --dist dist --version 26.2
```

该工具使用独立 `minecraft/v2-demo` 目录和 demo 参数；版本/JRE 路径不同需显式调整。客户端通过标准必须包含实际窗口和场景渲染、鼠标键盘输入、尺寸变化、音频输出、资源装载和正常退出；需要记录日志和可观察结果。依赖扫描成功、`java -version` 成功或部分 LWJGL 装载均不能替代这些验证。当前尚无完整客户端通过记录。

第三轮 init 预热、CPU/JIT/SIMD 分析及完整性能边界见 [启动性能第三轮报告](startup-performance-round3-2026-09-22.md)。

通用 init 预热池、按需补池、不可变映射优化，以及 Java main/完整退出的独立计时和回归结果见 [启动性能第四轮报告](startup-performance-round4-2026-09-23.md)。

复用不可变快照的 ELF 写时复制视图、最终包的 20 次版本启动及 Minecraft 首帧边界检查见 [启动性能第五轮报告](startup-performance-round5-2026-09-23.md)。
