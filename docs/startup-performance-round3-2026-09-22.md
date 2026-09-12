# 第三轮启动优化：init 管理预热（2026-09-22）

已实现 init 管理的一次性原生 worker 预热，并保留原来的普通启动入口。Java `-version` 普通完整启动的配对测试中位数从 354.78ms 降到 295.34ms，减少 16.8%。预热后部分批次低于 200ms，但扩大采样后尾部仍超过目标，不能宣布“稳定 200ms 内”或“大型应用已全部 200ms 内启动”。本轮没有把预热成本算成零，也没有以版本输出代替应用窗口可用性。

## 实现与边界

init 通过 `--prewarm-root ROOT --prewarm-dist DIST -- /program args` 创建并持有一个 worker。worker 先定位 runtime、打开认证会话、加载并绑定原生 providers，然后向 init 报告 `MarkPrewarmReady`，阻塞于 `AwaitPrewarmActivation`。只有同一原生控制进程认证后的 `ActivatePrewarm { pid: 1 }` 能放行。放行前没有加载应用 ELF，没有运行应用的 IFUNC、构造函数、Java VM 或 main。

每个 worker 只启动一次应用，不复用执行过应用代码的进程。fork 子进程和 exec 替代进程不继承预热入口；取消、init 退出及控制进程死亡均通过 init 的 Job 清理 worker。控制连接 EOF 本身既不释放屏障，也不等同于进程死亡。准备阶段的 worker 异常退出会唤醒等待方。

普通启动的两项优化也已直接写入源码：worker/helper 先只检查常规 runtime 文件，完整 provider 发现与校验仍由 runtime 执行，避免重复扫描；非标准文件名保留发现回退，普通 ELF 文件仍交给 ELF loader。标准流全部重定向时使用 `DETACHED_PROCESS`，避免额外隐藏 conhost；任一标准流是控制台时保留原创建方式。测得普通 Java 的进程树由 5 个进程减少到 3 个，init 预热执行只有 init 和 worker 两个进程。

## 测量结果

环境为本机 Windows、i7-13700H、Release 构建、Linux Temurin 25.0.4.1+1。度量为 `java -version` 从激活 RPC 开始，经过 JVM 执行和退出，直到 init 关闭且输出管道读完。文件系统缓存未清空。普通启动的基线与候选逐轮交替，预热数据是独立采样，不能用不同批次直接归因某一参数的效果。所有下列成功计时均校验退出码和版本标记。

| 场景 | 次数 | 中位数 ms | P95 ms | 最大 ms |
|---|---:|---:|---:|---:|
| 普通完整启动，优化前 | 10 | 354.78 | — | 383.49 |
| 普通完整启动，优化后 | 10 | 295.34 | — | 346.73 |
| 预热完成立即激活 | 20 | 212.80 | — | 287.91 |
| 预热完成，等待 100ms 后激活 | 20 | 199.46 | 249.34 | 270.94 |
| 预热完成，等待 1 秒后激活 | 20 | 222.85 | 340.36 | 376.99 |
| 另一次等待 1 秒的小样本 | 3 | 176.20 | 176.73 | 176.73 |

100ms 等待组的预热准备中位数为 130.22ms，包含准备、等待、执行和清理的总计中位数为 435.14ms。1 秒等待组的准备中位数为 141.50ms。等待时长是测试参数，生产控制方可在准备就绪后任意时刻释放屏障；它不是代码里增加的延迟。不能用 3 次较快结果覆盖 20 次较慢结果。本机同时存在其他高 CPU 进程，负载与计时有波动，但没有足够调度跟踪证据把所有长尾都归因于它们。

Firefox `--version` 另测 3 次，激活至清理中位数为 1182.65ms，最大 3279.54ms。它仍明显超过 200ms；这一结果也不能代表网页或 GUI 已可用。

原始结果和分发包 SHA-256：

- `artifacts/startup3-regular-java-final/results.json`
- `artifacts/startup3-prewarm-java-20/results.json`（早期脚本尚未记录文件哈希）
- `artifacts/startup3-prewarm-java-final/results.json`
- `artifacts/startup3-prewarm-java-idle-20/results.json`
- `artifacts/startup3-prewarm-idle-cpu/results.json`
- `artifacts/startup3-prewarm-firefox/results.json`

两次早期带 JVM 参数的命令被 PowerShell 拆分了未加引号的冒号参数，JVM 拒绝启动；`startup3-initial-cpu` 和 `startup3-prewarm-java-runtime` 不作为成功性能数据。复测使用带引号的参数，实际通过记录为 `startup3-prewarm-java-jit`。

## 热点、CPU、等待、JIT 和 SIMD

新增 `KINAKAZE_STARTUP_PROFILE` 可记录各阶段墙钟、线程 CPU、进程 CPU 和线程周期数；默认关闭，不查询 CPU 时钟或写日志。`benchmark-startup.py --cpu-metrics` 与预热脚本另记录整个受管 Job 的 CPU、进程数、缺页和 IO 字节，包含已退出子进程。预热脚本还分开记录准备 CPU、闲置 CPU 和激活阶段 CPU。

早期普通启动样本中，worker 首次完整 provider 发现约 40–45ms，runtime 随后再次发现约 23–30ms，绑定约 32–42ms。新路径的 worker runtime 定位样本约 0.56ms；后续原生 DLL 加载仍有成本，完整 provider 校验保留。预热将这部分工作放在激活之前。

激活后的热点仍在 JVM 与 ELF loader：示例中 libjvm 映射约 21.6ms，准备补丁约 23.8ms，其中完整内容哈希在此前样本中约 12ms；JVM 自报 Create VM 约 68–89ms。普通 supervisor 的“等待 guest 退出”覆盖整个 guest 工作，不是白白浪费的同等时长。

1 秒等待的 20 次采样中，闲置阶段 Job CPU 增量全部为 0；激活阶段 CPU 中位数合计 250ms，其中用户态 78.125ms、内核态 171.875ms，约 36,232 次缺页。不同字段的中位数不保证可以直接相加。另一次较快 3 次采样的激活 CPU 中位数为 187.5ms。这里的 CPU 是多线程累计值，可以大于墙钟；缺页包含软缺页，IO 字节包含缓存 IO，均不能直接推导物理磁盘等待时间。CPU 时间计数粒度较粗，0 表示未测到计数增量，不表示数学意义上绝无 CPU 指令。

尝试 WPR CPU 跟踪时系统返回 `0xc5585011`，当前 profiling privilege 不可用，因此没有声称取得 ETW 火焰图或精确的 off-CPU 等待栈。本轮使用上述阶段计时、周期计数和 Job 统计作为证据。

实际 Linux JVM 的 `PrintFlagsFinal` 输出显示 `TieredCompilation=true`、`UseAVX=2`、`UseSSE=4`；CDS 日志显示归档区域和堆数据成功映射。`JavaRuntimeProbe::sum` 的编译日志实际出现第 3、4 层编译，随后线程、文件读写和 8 次子进程调用全部通过。没有关闭 JIT、降低最终编译层级或关闭功能以换取启动数字。BLAKE3 构建输出确认包含 SSE2/SSE4.1/AVX2 汇编及运行时派发；原有 SIMD 完整内容哈希仍保留，没有改成时间戳或抽样校验。

这些证据支持继续检查 libjvm 映射、内核内存操作和 JVM 初始化成本；仅继续提前原生 provider 加载，尚不足以证明所有负载下都能达成 200ms。

## 使用与验证

以下命令在仓库根目录运行，启动由 init 管理的一次性预热 worker，再释放屏障。脚本捕获 stdout/stderr，给应用 stdin 提供 EOF，具有超时及进程树清理；它不是交互终端启动器，也不是自动补充的 worker 池。

```powershell
python tools/prewarm-startup.py --root artifacts/guest-root --dist artifacts/startup3-final-dist --output artifacts/my-prewarm-run --repeat 5 --hold 1 --expect 'openjdk version' -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version

python tools/test-prewarm.py --root artifacts/guest-root --dist artifacts/startup3-final-dist --output artifacts/my-prewarm-lifecycle
```

`--profile` 同时开启 loader 与新启动计时；性能数字应使用关闭 profiling 的采样。控制协议采用原有长度限定帧、版本号、凭证和原生进程身份认证。前端可以使用 `AwaitPrewarmReady` / `ActivatePrewarm` / `AwaitExit` 接入，而无需让应用执行任何准备命令。

验证结果：199 项相关 Rust 测试通过、0 失败，另有 3 个既有 helper/忽略项；4 项真实 init 预热生命周期测试通过，包含释放前无文件副作用、EOF 不激活、退出码/标准流、取消清理、准备失败和控制进程死亡。实际 Java JIT/线程/文件/8 次 spawn、Node 两项行为测试、Python runtime、pthread cleanup、启动接口、allocator interposition/IFUNC、浏览器 ABI、Firefox glxtest，以及原生 worker smoke 均通过。没有宣称整个工作区所有测试都通过。

测试记录位于 `artifacts/startup3-tests`、`startup3-prewarm-lifecycle-final-lock`、`startup3-prewarm-java-jit`、`startup3-runtime-tests`、`startup3-browser-tests`，以及 `startup3-node-report.json`、`startup3-smoke.log`。

为避免其他任务同时编译/修改源码污染性能对比，使用 `artifacts/startup-source` 的固定源码背景构建基线和候选，本轮改动同步到该快照；发布测试包为 `artifacts/startup3-final-dist`。本轮所负责的 Rust 文件与已测试快照逐字节一致。其他任务随后对 ELF 控制流、TLS、分配器或桌面库的修改不属于这组固定基线的性能结论；源码工作区中的这些修改均保留。
