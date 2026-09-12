# 收束检查点：2026-09-12

> 2026-09-16 已按用户指令恢复持续推进。本文保留上一阶段检查点；新增功能、验证结果及当前工作入口见 [持续推进记录](progress.md)。

V2 已迁移为独立 Git 仓库 `F:\crysoacu2`，本地构建产物保留但由 `.gitignore` 排除；所引用的总体设计和 syscall 清单已复制到本仓库 `docs/`。外部 Linux 程序源路径可通过 `--source` 指定。

用户要求因额度开始收束，因此停止扩展功能。此检查点不是完整 Linux ABI 或 Minecraft 客户端完成声明。

## 已实现并验证

- 独立 `kinakazev2/` 工程；自研 libc、loader、VFS、TLS、分配器和 provider 实现以 rlib 只链接到唯一 runtime DLL。28 个 DLL 各对应真实 ELF so，完整目录包含 4,244 个导出。
- 一个活动 Windows worker 对应一个 Linux Process，线程留在该 worker；manager 维护逻辑 PID、fork/exec 事务和退出。exec 在旧 native 进程全部线程死亡后激活候选，IFUNC 在激活之后执行。
- 最终 `tools/build.ps1` 完整通过：89 项 Rust 测试通过、2 项忽略、15 项 catalog 测试通过，管理 smoke 验证退出后对象/进程/事务均为 0。
- 实际 Linux BusyBox shell 管道、重定向、文件复制/比较/删除通过。
- 最终 dist 的 LWJGL 3.3.3 OpenGL 探针创建窗口、编译 shader、绘制三角形，输出 `LWJGL_SMOKE_OK` 并退出 0；使用 NVIDIA RTX 4060 的 OpenGL 3.2。源码在 `tests/guest/LwjglSmoke.java`。
- 实际 Linux curl：9 项 HTTP/TLS/解析正反向验证通过，最终 dist 再次通过，见 `artifacts/final-curl-report.json`。
- Linux Java 25：JIT、8 个异步任务、UTF-8 文件操作、默认 ProcessBuilder 连续 8 次输出与退出状态通过，最终 dist 再次通过，见 `artifacts/final-java.stdout.log`。
- 实际 fork/exec/wait/spawn、跨 worker 内存读写、SDK nestedfork Copy/Share/Reset，以及 3 个忙线程加 1 个阻塞线程的 exec/IFUNC 交接通过。
- 客体准备工具解析 5,293 文件和 306 条依赖边；缺失 libXtst 使用固定版本官方 Debian ELF，校验包、文件哈希及版权文件。

## 主要复现入口

在 `F:\crysoacu2` 仓库根目录运行：

```powershell
./tools/build.ps1
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --offline
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/bin/busybox echo hello
python tests/guest/curl-smoke.py --worker target/debug/kinakaze-worker.exe --root artifacts/guest-root --dist dist --report artifacts/curl-smoke-report.json
javac --release 17 -d artifacts/guest-root/tests/java tests/guest/JavaRuntimeProbe.java
./target/debug/kinakaze-worker.exe run --root artifacts/guest-root --dist dist -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -cp /tests/java JavaRuntimeProbe spawn
python engine/tests/guest/run-exec-thread-probe.py
# 旧的向 libc 注入 SDK 状态导出的 fixture 已撤除；管理状态用 worker smoke，guest fork 用实际 ELF 探针分别验收。
python tools/run-minecraft.py
```

首次无依赖缓存时去掉 `--offline`。输入文件来自已有本地 Linux 程序/JRE/Minecraft 安装，工具不会修改原始目录或复制原始世界和账户数据。Minecraft 默认在 `minecraft/v2-demo` 运行官方 demo 模式。

## 继续工作的位置

最终 dist 已成功装载 Minecraft 的 LWJGL、GLFW、OpenAL、shaderc、SPIRV-Cross、VMA、Freetype 等原生依赖，并进入客户端 Main/Datafixer 初始化。35 秒有界检查在出现主菜单验收证据前结束，由测试器终止所属 supervisor，不能据此宣称客户端可玩或确定启动失败。下一步用 `python tools/run-minecraft.py` 运行更长时间，检查主菜单、画面、输入、音频、世界保存与正常退出。日志在 `artifacts/final-minecraft.stdout.log` 和 `artifacts/final-minecraft.stderr.log`；另有 workdir FileStore 查询的 EINVAL 警告和 /etc/os-release 缺失提示。

此前 GLFW 的 posix_fallocate 缺失和 Xlib 符号误分组均已修复，独立 OpenGL 探针已通过。OpenAL 最后一次独立播放测试发生在 pthread 调度入口纳入之前，停在 libopenal 装载；最终 Minecraft 装载该库成功，但尚未验证实际 PCM 输出。没有遗留 V2 测试 worker/init 进程。

完整 ABI 的未完成范围包括：credentials/dumpable/ptrace 权限模型、全部调度策略、异步信号交付、guest 栈展开/cleanup、DSO 卸载引用、AIO、录音、ALSA mmap、全部 clone/namespace 组合及字体数据。当前 process_vm 跨进程访问限制在同 manager 域的自身或后代；这不是完整 Linux ptrace 权限实现。逐领域方案见 [ABI 路线](abi-roadmap.md)。

构建产物和运行日志均在忽略的 `target/`、`engine/target/`、`dist/`、`artifacts/`。完整外层构建/测试的最终结果写入 `artifacts/final-build.log`；旧专项结果不能替代改变源码后的最终回归。
