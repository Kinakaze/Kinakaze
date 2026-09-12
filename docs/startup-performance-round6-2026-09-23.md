# 第六轮：降低 Java 子进程成本，修复跨视图内存提交

本轮继续使用 init 管理的通用预热池。真实 Minecraft 26.2 在预热池激活后已经观测到首帧提交，但约需 23.8–23.9 秒；**大型应用 200ms 就绪目标仍未达到**。`java -version` 的短路径成绩不能代表 JVM 完整初始化或图形应用就绪。

## 热点与改动

在未改动 Java 工作负载的诊断运行中，Minecraft 在 34.65 秒提交首帧，期间记录 18,362 次类初始化。14 次 JVM 发起的 fork 各约复制 326MiB（中位数），单次 fork 中位数约 444ms，合计约 6.48 秒；应用期间仍有大量并行 CPU 工作，包括 Datafixer 初始化（日志约 2.25 秒）。这些计数说明 JVM 类初始化、JIT 及子进程复制都值得优化；它们不足以把其余时间归因于磁盘等待。ETW 栈采集在当前宿主权限下不可用。

普通 `posix_spawn` 过去复用完整 fork/exec，Java 每次启动辅助进程都复制父 JVM 的大量内存。现在只有绝对路径 ELF、无文件动作、且属性仅涉及可安全交接的信号集合时，才由 init 的既有身份事务创建全新挂起 worker，交接 exec 状态并激活；其余调用仍用通用 fork/exec。新路径保留 PID、wait、环境、文件描述符、信号掩码及默认处置。描述符交接前检查代数、标志和原生句柄；在原生子进程创建前遇到短暂 `EBADF`/`EAGAIN`，最多重新采样三次，然后回退通用路径。这样处理了压力测试中观察到的一次竞态，而不会让失败的候选子进程继续执行。

另外，HotSpot 会对跨越多个 Windows 视图的匿名私有地址范围做 `MAP_FIXED`。原实现仅能替换单视图，导致有空闲宿主内存时仍报告 `os::commit_memory` 的 `EIO`。现在先验证整段范围均归当前进程私有匿名映射，再在同一映射锁下按视图片段重置并提交，保留相邻页和写时复制关系。原有 `PROT_NONE` 片段处理仍保留。

## 性能和功能证据

冻结源码背景的 8 组 JavaRuntimeProbe `spawn` 配对测试中，完整退出中位数由 4003ms 降至 1886ms，约减少 53%；候选路径的子进程复制开销显著降低。该轮之后发现过一次文件描述符采样竞态，因此这些配对数值**不能当作最终集成包的稳定吞吐保证**。补上有界重试和回退后，在构建负载下连续 30 次完整 Java 探针（每次 8 个子进程，共 240 个）全部通过；对应完整退出中位数为 2615ms，宿主负载不同，不与早先基线直接相减。

跨视图内存探针在原包稳定复现 `EIO`，修复包通过跨两段 16MiB 视图的读写重置、邻页保持、fork 写时复制、`PROT_NONE` 再提交和占用/占位混合片段测试。新鲜进程探针覆盖信号、`FD_CLOEXEC`、共享文件偏移、umask、UNIX socket 权限传递、二次 exec 与嵌套 fork；既有 SpawnRuntimeProbe 覆盖文件动作、进程组及 `spawnp` 回退路径。

修复跨视图问题后的 Minecraft 配对样本，候选包两次首帧为 23.766/23.947 秒；基线一个样本因同一 `EIO` 提前退出，另一个在 30.698 秒首帧。另一次较早基线的两次首帧为 28.836/34.550 秒。样本少且宿主负载变化，能证明候选路径在此场景可运行并更早提交首帧，不能据此精确估计总体加速比或宣称菜单完全就绪。首帧口径是归属窗口的提交标记加 `DwmFlush`；屏幕抓取被其他窗口遮挡，已剔除，未作为渲染证明。

源码相关的 26 组 Rust 测试目标全部通过，包括 exec 继承栅栏、fork 交接、spawn 和内存映射。冻结包的通用池生命周期、Java 压力测试和 C 探针通过。**合入当前工作区后的发布包** `artifacts/startup6-integrated-dist` 也构建成功，16 项通用池生命周期用例、Java 8 次子进程调用连续 10 轮（共 80 个子进程）、新鲜进程、既有 SpawnRuntime 和跨视图探针均通过。集成包的 Java 子进程探针完整退出中位数为 1288ms；20 次预热 `java -version` 均低于 200ms，退出中位数 108.3ms、最大 174.3ms，池准备另耗 73.1ms。一次真实 Minecraft 运行在激活后 **18.672 秒**提交首帧，池准备另耗 64.2ms。此次更快的单次结果不能作为稳定分位数，也未确认菜单完全可交互。

测试源码在 `tests/guest/SpawnFreshProbe.c` 与 `tests/guest/MmapCrossViewProbe.c`；原始日志及时间记录在 `artifacts/startup6-*`，集成包运行报告为 `artifacts/startup6-minecraft-paired/integrated-final.json`、`artifacts/startup6-integrated-java-10/results.json` 和 `artifacts/startup6-integrated-version-20/results.json`。构建使用 `./tools/build.ps1 -Release -SkipFormat -SkipTests`；Rust 测试在冻结源码背景另行运行，集成包完成的是上述真实功能回归。

补充正常启动检查：集成包再次在 19.527 秒提交首帧，窗口标题为 `Minecraft 26.2`，之后持续存在 20 秒。测试器发出 Windows `WM_CLOSE` 后又等待 20 秒，窗口和进程仍存在，因而该次完整关闭检查失败。`WM_CLOSE` 在当前显示层仅是转交给 Linux 客户端的请求；这条结果不能单独定位失败在事件转发还是应用未处理，但**不能据首帧宣称 Minecraft 已可正常进入并操作菜单**。该测试使用演示账号占位令牌，日志中的在线账户/Realms 401 不证明本地图形启动失败。记录为 `artifacts/startup6-minecraft-paired/normal-launch-check.json`。

完整目标仍需显著缩短应用本身的类初始化、JIT、资源和图形准备，并确认菜单交互与正常退出。通用池能把 worker 建立成本前移，但不能在未指定应用与状态时预先执行 Minecraft 的全部初始化，也不能把数十秒的真实工作计为 200ms 内完成。
