# Java 启动速度（2026-09-26）

当前版本启动一个全新 Linux JVM，最小程序进入 `main` 的宿主可观察耗时中位数为 **126.59ms**，P95 为 **136.58ms**。相对最初版本累计减少 **18.2%**，相对上一轮减少 **6.8%**。

## 测量条件

- 实际 Linux Temurin OpenJDK **25.0.4.1+1 LTS**，64-Bit Server VM，JVM 默认参数；三个版本使用同一 JRE、guest root、class 文件。
- `original`：`artifacts/startup-extreme-20260926/baseline-dist`。
- `previous`：`artifacts/startup-extreme3-20260926/final2-dist`，已有按需符号绑定。
- `current`：`artifacts/startup-extreme4-20260926/final-dist`，进一步按需构造模块元数据。
- 每个场景、每版 20 次正式测量和 2 次预热；三个版本轮换执行位置。每次重新创建 supervisor、init、worker 和 JVM，没有复用预热 JVM 或常驻 init，没有增加提前绑定。
- 耗时从宿主发起启动开始，包含宿主进程和兼容层开销。“到 main”由宿主读取到 Java 首条输出 `JAVA_MAIN_ENTERED` 计时，包含管道传递及读取线程调度，不是 JVM 内部时钟。
- `JavaStartupProbe` 第一条语句输出标记，随后确认 guest `os.name=Linux`；`javac` 编译不计入启动时间。
- 操作系统文件缓存已热，不代表机器冷启动。复测三个场景期间宿主 CPU 平均占用约 21%–26%。

## 同场结果

单位：毫秒，中位数。

| 场景与指标 | 最初版本 | 上一轮 | 当前版本 |
| --- | ---: | ---: | ---: |
| 最小程序：启动到 main | 154.79 | 135.85 | **126.59** |
| 最小程序：启动到退出 | 174.91 | 156.00 | **145.31** |
| `java -version`：启动到退出 | 182.46 | 159.39 | **153.75** |
| JavaRuntimeProbe：启动到 main | 155.47 | 139.16 | **130.93** |
| JavaRuntimeProbe：完成 JIT/线程/文件测试后退出 | 244.86 | 226.03 | **220.83** |

JavaRuntimeProbe 运行真实 JIT 循环、8 个异步计算任务和临时文件读写。这里没有传 `spawn`，因此不包含其额外 8 次 ProcessBuilder 子进程启动。最后一行是功能探针整体运行时间，不能当成纯启动耗时。

最小程序“到 main”的 P95 为：最初 **164.91ms**、上一轮 **142.26ms**、当前 **136.58ms**。本次仅 20 个样本，不能据此保证长期尾延迟。

复测 **180 次正式启动全部通过**，均校验正常退出和对应输出标记。

## 宿主干扰与复现

第一组 `paired` 期间存在宿主编译负载，`java -version` 和最小程序期间宿主 CPU 平均占用约 62%、52%，结果波动很大。负载回落后完整重跑三个场景和三个版本，以上结果全部取自 `paired-repeat`，没有跨批次挑选最快样本。第一组原始记录也保留。

工件位于 `artifacts/java-startup-20260926`：

- `run.py`：独立进程、轮换顺序、到 main/退出计时、CPU 及发布包 SHA-256 记录。
- `JavaStartupProbe.java`：最小入口程序。
- `paired-repeat/results.json`：复测逐次数据和汇总；各场景子目录保存每次 stdout/stderr。
- `summary.json`：启动指标的相对收益。
- `paired`：受干扰的首次批次。

复现命令（需要本机已有 Java guest root 和 javac）：

```powershell
python artifacts/java-startup-20260926/run.py --tag paired-repro
```

本次只新增基准脚本和报告，没有修改生产运行时代码。
