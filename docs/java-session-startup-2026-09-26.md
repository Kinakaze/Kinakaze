# Java 在已有 init session 中的启动性能

保留每个 Linux 环境一个独立 init.exe 的架构。外部再次启动通过 `init launch --session-file` 通知原会话，内部 fork/exec/ProcessBuilder 也继续使用原管理器；只有新建环境才创建新的管理器。本轮合并 init 与 CLI 的实验已撤回，未进入最终源码或发布工件。

同场复测，外部客户端启动全新 Java 到 `main` 的中位数从 **95.90ms 降至 85.52ms，减少 10.8%**；本机为 **44.75ms**，仍是 **1.91×，没有达到 1×**。

## 本轮修改

`apps/init/src/pool.rs` 原先在应用消耗一个空闲 worker 后，最多等待 250ms 再补充，以避免准备替代 worker 与当前应用启动竞争。但应用已经退出时，这个等待仍会保留，下一次外部请求可能需要现场等待 worker 准备。

现在，init 在原有退出通知唤醒后检查逻辑进程：如果剩余进程全是尚未执行应用的就绪 worker，立即补充缺位。仍有应用、保留的 worker 或准备失败时保留原有调度和失败退避。检查逻辑进程而非仅看宿主进程退出，避免把 exec 交接时旧 worker 的退出误判成整个应用结束。

池容量保持 1，没有增加提前绑定。原有通用 worker 的准备只加载宿主基础设施及模块目录；应用在激活时才确定。实际跟踪确认空闲就绪阶段没有客体 ELF 加载记录，没有 `provider-symbol` 解析记录。没有提前加载 libjvm，没有预热或复用 JVM，没有修改 JVM 参数。

收益来自提前完成本来就需要做的通用 worker 补充，属于 session 调度优化，不是 JVM 内部执行变快。

## 测量方法与结果

- 本机：Windows Oracle HotSpot 23.0.1；项目：Linux Temurin HotSpot 25.0.4.1。同一份 `javac --release 17` 字节码。版本和操作系统不同，不能把全部差值归因于兼容层。
- 同场比较 native、旧 session、新 session，各 30 次正式运行、3 次预热，轮换执行顺序，无人为请求间隔；两个项目版本各有自己的独立环境，每个环境始终保持同一个 init 管理器。
- 每次外部启动都实际执行 `init.exe launch --session-file ... --wait`。这个短命 init.exe 进程是客户端，不是新建的管理器。计时包括客户端创建、认证、池等待、激活、JVM 初始化，到宿主读到首条 `JAVA_MAIN_ENTERED`；退出指标还包括等待最终逻辑进程退出。
- 操作系统文件缓存已热。池准备不执行客体程序，每次实际 JVM 都是新进程；已执行的 worker 不会被回收再执行另一应用。
- 会话创建成本单独记录，不放进已存在环境的每次启动时间。复测新/旧 init 到 READY 分别为 26.32/19.35ms；READY 不是池就绪。首轮新包首次启动为 165.13ms，原始数据保留，不能用本报告推断环境冷启动已达本机速度。

复测 `paired-repeat`，宿主平均 CPU 20.05%。单位：ms。

| 指标 | 本机 | 旧 session | 新 session |
| --- | ---: | ---: | ---: |
| 到 main 中位数 | 44.75 | 95.90 | **85.52** |
| 到 main P95 | 49.52 | 116.48 | **95.88** |
| 到退出中位数 | 60.48 | 112.10 | **105.43** |

首轮 `paired-refill` 同样每侧 30 次，宿主平均 CPU 20.11%：本机 42.52ms、旧 session 95.72ms、新 session 82.89ms，新版减少 13.4%，相对本机 1.95×。所有样本均保留；上述复测没有与前几轮新建环境的结果跨批次相减。

### 已有 Java 父进程内部启动子 JVM

另用同一份 Java 父程序，在各平台通过 `ProcessBuilder` 连续创建全新子 JVM。每侧三批，每批先排除三个预热子进程，再测十个；计时从父进程输出启动标记到宿主观察到子 JVM main，父 JVM 的初次启动不计入。

这组测试宿主平均 CPU 34.74%，本机到 main 中位数 **60.04ms**，项目 **131.36ms**，为 **2.19×**；到父进程确认子进程退出分别为 77.08/147.03ms。它经过 JVM 自身的子进程创建和 exec 路径，与外部 session 客户端场景不同，也处于不同负载窗口，不能直接相减。

## 剩余开销

独立开启 profiling 的六次新 session Java 启动中，libjvm 中位数：完整不可变镜像快照 **9.44ms**，完整内容哈希 **2.10ms**，AOT 缓存校验 **1.08ms**，指令补丁应用 **4.81ms**。这些仍发生在每个全新 JVM 的启动路径上。`prepare` 合计 8.84ms 已包含哈希和 AOT 各项，不能重复累加。

通用 worker 自身的 runtime bootstrap 约 5.39ms、provider 准备约 7.42ms，可在 session 空闲期间完成。日志里的 `providers-bind` 是历史阶段名称，空闲阶段的实际跟踪没有客体符号绑定。

这不是完整的差值归因：JVM 初始化、版本差异、平台调用和子进程创建路径仍有额外影响。当前没有证据支持宣称已实现 1×。

## 验证与工件

- init/manager Rust 测试：**58 通过、1 个现有子进程入口忽略**。
- 实际池生命周期测试：**16 项通过**，覆盖 Java JIT/线程/文件/ProcessBuilder、并发应用、环境隔离、补充、worker 崩溃及关闭竞态。
- session 专项：**6 项通过**，覆盖空闲无客体绑定、重连、父 PID/cwd/env/退出码、非法父进程后释放保留项、内部派生仍归原管理器、客户端断开后应用存活及失效 session 拒绝。
- 两轮外部比较 **180 次正式启动通过**；内部子 JVM 比较 **60 次正式启动通过**。
- 当前工作区 init/worker 的 `cargo check --locked` 通过。
- 第一版专项脚本尝试了该精简 root 中不存在的 `/bin/sleep`，已改用现有 `/bin/busybox sleep`；失败日志和修正后通过的日志都保留。

工件目录：`artifacts/java-session-20260926`。

- `final-dist/`：验证后的发布包。68 个文件与 `candidate-dist/` 逐一哈希相同，相对上一轮发布包只有 init.exe 改变。
- `source/`、`before/`、`build.log`、`final-dist-manifest.json`：固定源码、修改前池实现、构建记录和哈希清单。
- `compare_sessions.py`、`paired-refill/results.json`、`paired-repeat/results.json`：外部 session 启动比较。
- `JavaTreeStartup.java`、`compare_tree.py`、`internal-tree/results.json`：内部 ProcessBuilder 比较。
- `audit_session.py`、`session-audit/results.json`、`session-audit/before-activation.log`：单管理器和按需加载证据。
- `pool-regression/results.json`、`unit-tests.log`、`live-check.log`：功能验证。
- `profile-candidate/phase-summary.json`：独立 profiling 汇总，不用于正式耗时表。

复现外部比较：

```powershell
python artifacts/java-session-20260926/compare_sessions.py --dist artifacts/java-session-20260926/final-dist --previous artifacts/java-near-native-20260926/final-dist --repeat 30 --warmup 3 --tag repro
```

本报告与之前每次 `worker run` 新建 Linux 环境的冷进程树报告使用不同场景，二者应分别保留。
