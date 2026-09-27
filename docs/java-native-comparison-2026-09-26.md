# Java 与本机 Windows 原生启动对比（2026-09-26）

本次同场对照，当前项目运行 Linux Java 的最小程序进入 `main` 中位数为 **114.25ms**；本机 Windows Java 为 **41.60ms**。项目耗时约为本机的 **2.75 倍**，多 **72.64ms**。

## 环境与边界

- 本机：`E:/APPD/JDK23/bin/java.exe`，Oracle Java/HotSpot **23.0.1+11-39**，Windows x64。
- 项目：Linux Temurin/HotSpot **25.0.4.1+1-LTS**，发布目录 `artifacts/startup-extreme4-20260926/final-dist`。
- **JDK 版本、发行商与操作系统不同**。这组测量反映用户当前安装环境的实际差距，不能将全部差值解释为同版本 JVM 的兼容层净开销。
- 两边执行同一份 `.class` 文件，使用 `javac --release 17` 编译；宿主 classpath 直接指向 guest root 对应的同一物理目录。最小程序和功能探针只按命令行参数校验预期 OS，字节码完全相同。
- JVM 默认参数，没有常驻 JVM、进程池或新增提前绑定。每次创建全新进程；本机为 1 个 JVM 进程，项目为 supervisor/init/worker 3 个宿主进程。
- 计时从宿主创建启动任务开始，包含 Job 建立、进程创建、项目管理进程启动、JVM 初始化。“到 main”使用首条 stdout 标记在宿主被读取的时刻，包含管道与读取线程调度。
- 每场景每侧 **30 次正式测量、3 次预热**，交替执行。操作系统文件缓存已热，不是机器冷启动。三个场景宿主 CPU 平均占用约 15%–23%。

## 结果

单位：毫秒，中位数。

| 指标 | 本机 Windows Java | 当前项目 Linux Java | 多耗时 | 耗时比 |
| --- | ---: | ---: | ---: | ---: |
| 最小程序：进入 main | **41.60** | **114.25** | +72.64 | 2.75× |
| 最小程序：启动到退出 | 53.32 | 131.51 | +78.19 | 2.47× |
| `java -version`：启动到退出 | 54.50 | 132.50 | +77.99 | 2.43× |
| JIT/线程/文件探针：进入 main | 43.37 | 118.15 | +74.77 | 2.72× |
| JIT/线程/文件探针：完成并退出 | 119.63 | 201.84 | +82.22 | 1.69× |

最小程序到 main 的 P95：本机 **43.69ms**，项目 **119.71ms**。

180 次正式运行全部通过。功能探针验证 JIT 计算、8 个异步计算任务、文件写入/读取/路径身份检查，没有执行其可选 ProcessBuilder 分支。它是短功能负载，不用于推断长期 Java 吞吐。

在这个功能探针中，整体多耗约 82ms，而到 main 已多耗约 75ms，因此差距主要出现在启动阶段。这里并未进一步拆分 JVM 版本差异、ELF 加载、宿主进程管理等各自贡献。

前一份 [Java 启动报告](java-startup-performance-2026-09-26.md) 测得当前到 main 约 127ms；本次宿主负载较低且使用跨平台入口，测得约 114ms。比较应使用本次同场的两侧数据，而不是跨批次相减。

## 复现和原始数据

```powershell
python artifacts/java-startup-20260926/compare_native.py --tag native-comparison-repro
```

工件：`artifacts/java-startup-20260926`。

- `compare_native.py`：本机与项目交替测量脚本。
- `JavaCompareStartup.java`、`JavaCompareRuntime.java`：跨平台入口及从原 guest 功能探针派生的对照程序；生产测试源文件没有改动。
- `native-comparison/results.json`：逐次数据、命令、Java 版本、class/JVM/关键发布文件 SHA-256。
- `native-comparison/summary.json`：中位数、P95、耗时差与倍数。
- 各场景子目录：每次启动的 stdout/stderr。

本次未修改生产运行时代码或本机 JDK 安装。
