# 大型应用启动性能实测：2026-09-22

已直接修改执行引擎，生成并验证 `artifacts/startup-optimized-dist`。优化集中在 ELF 指令准备，保留完整文件内容校验、缓存站点验证、TLS/系统调用语义和 fork 映射登记。

下面是关闭分析日志后的整次启动耗时中位数，包含 Windows worker、init、Linux 程序执行和退出。每轮交替两个构建的运行顺序，每个构建预热一次；要求退出码为 0 且输出指定成功标记。浏览器项目测量 `--version` 的装载启动，不代表页面、主窗口或首帧已经可用。

| 场景 | 优化前 | 优化后 | 耗时减少 | 每个构建的有效样本 |
| --- | ---: | ---: | ---: | ---: |
| Linux Java 25 `-version` | 430.4 ms | 343.2 ms | 20.3% | 7 |
| Java JIT、线程、中文文件读写探针 | 582.7 ms | 472.4 ms | 18.9% | 5 |
| Node 加载 VM 并校验计算结果 | 582.5 ms | 499.4 ms | 14.3% | 5 |
| Firefox `--version` | 9,242.2 ms | 1,286.8 ms | 86.1%，约 7.18 倍 | 5 |
| Chrome `--version` | 6,764.8 ms | 1,160.2 ms | 82.8%，约 5.83 倍 | 5 |
| Java，无有效 AOT 缓存 | 922.8 ms | 784.6 ms | 15.0% | 3 |

最后一行使用本次单独准备的 `startup-java-cold-root`，每轮归档其 AOT 目录后再运行；没有清空操作系统文件缓存，因此不称为整机冷启动。其余行使用有效 AOT 缓存。汇总为 `artifacts/startup-performance-summary.json`；原始报告为 `startup-paired-{java,java-runtime,node,firefox,chrome}/results.json` 和 `startup-cache-validation/summary.json`。报告记录完整命令、root、每次耗时、退出码、输出标记以及发行文件 SHA-256。

## 瓶颈与修改

初始 Java 分析中，`libjvm.so` 映射约 18 ms，重定位约 1.6 ms，热缓存下的指令准备约 118 ms；无有效缓存时准备阶段约 569 ms。主要成本在执行适配层。

- **完整文件哈希**：原 FNV-1a 每个字节都依赖上一次乘法结果。改为具有运行时 SIMD 选择的 BLAKE3，仍覆盖全部 ELF 字节，使用完整 256 位摘要。键格式自然隔离旧缓存；新增测试检查中间字节、长度变化和文件名处理。[BLAKE3 实现](https://github.com/BLAKE3-team/BLAKE3)
- **跳板邻近分配**：原来每增加一个 64 KiB 块，就从指令附近重新逐个地址尝试 `VirtualAlloc`。改为 `VirtualAlloc2` 地址约束，从较近范围逐步扩大，最后保留原有精确地址回退；同时约束整个分配块处于 rel32 范围并保持 fork 登记。边界测试覆盖低地址、最高用户地址和溢出。[Windows 地址约束](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-mem_address_requirements)
- **装载期间批量发布**：AOT 的源映射本来就可写且尚未进入客体执行。通过显式 `UnpublishedCode` 范围检查，省去每个站点重复的权限切换和刷新，整段完成后检查一次指令缓存刷新结果。VEH 的运行中修补仍走原有保护与刷新路径。[Windows 指令缓存要求](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-flushinstructioncache)
- **跳板空间与查找**：优先查找最近的分配块；编码后按实际长度、64 字节对齐回收当前预留的空余尾部。只有它仍是该块最后一次预留时才回收，不能跨过并发调用者的空间。保留按整块登记、复制和恢复的 fork 行为。新增测试覆盖中途出现另一笔预留的情况。
- **首次解码的重叠检查**：指令按地址递增处理，以最近一个成功跳板的终点代替遍历所有历史 TLS 区间，消除站点数量的平方级成本。独立入口、直接分支目标和 syscall 尾部保护继续生效。

补丁生成期间还发现并修正了并行 GS 支持中的编码问题：Firefox 的 `mov gs:[rdx],ah` 不能被改写成需要 REX 前缀的高编号基址寄存器。GS 翻译为 AH/BH/CH/DH 选择可编码的基址，新增 8 种加载/存储编码验证。该兼容性修复同时包含在两份性能对照构建中。

最终另做的一次带分析日志的阶段对照如下。这些诊断数据不混入上面的总耗时样本：

| 映像/阶段 | 对照 | 优化 |
| --- | ---: | ---: |
| libjvm.so，完整内容哈希 | 37.9 ms | 10.6 ms |
| libjvm.so，应用缓存补丁 | 108.8 ms | 12.3 ms |
| libxul.so，完整内容哈希 | 216.0 ms | 52.3 ms |
| libxul.so，应用缓存补丁 | 9,544.6 ms | 476.7 ms |

诊断入口沿用 `KINAKAZE_LOADER_PROFILE`，新增 `execution-<pid>.log`，区分哈希、缓存读取、验证、应用和首次解码；未设置时不写日志。仍有 Windows 进程/管理会话建立、ELF 映射、首次控制流分析和应用自身初始化成本，本轮没有将这些剩余时间算作已经消除。

## 对照有效性

工作区同时有其他兼容性修改，因此最终对照来自固定的本地源码快照。`startup-control-dist` 保留相同兼容性代码，恢复原完整 FNV 哈希、逐站点保护、邻近地址扫描、1 KiB 固定跳板预留和历史区间遍历。原跳板文件取自 Git blob `ba4ebd291aacaf236b6e33fc67dc74f8b2ea5f95`；两边保留相同 GS 支持与高位寄存器修复。

逐文件哈希比对确认，两个包只有 `kinakaze_guest_engine.so` 及链接该引擎的 `libkinakaze-runtime.so.1` 不同。worker、init、libc、pthread、VFS、链接器、图形等其余文件完全一致。证据为 `artifacts/startup-controlled-binary-differences.json`。现场源码快照、恢复对照的脚本及构建日志保留在本地 `artifacts/startup-*`；这些被忽略的本机产物不随新检出提供。

## 功能验证与边界

| 验证 | 结果 | 证据 |
| --- | --- | --- |
| 执行引擎单元测试 | 38 通过，1 个原有性能诊断忽略；包含真实 TLS 跳板执行、系统调用桥、地址范围、空间回收和缓存键 | `startup-native-tests/kinakaze_guest_engine.log` |
| Java 默认 JIT、线程、中文文件及 ProcessBuilder | 最终构建连续 3 轮通过，每轮实际派生 8 次并校验合并输出和退出码 | `startup-final-java-spawn/results.json` |
| Node 运行行为 | 两个行为探针通过，含 VM、线程、异步 I/O、子进程及网络 | `startup-node-regression.json` |
| Python、pthread cleanup、启动 ABI、allocator interposition/IFUNC | 5 个真实 Linux 探针通过 | `startup-compatibility/results.json` |
| 装载、TLS、构造器和两代 fork；原生数学恢复 | `LOADER_ENTRY_OK`、`NATIVE_MATH_OK` | `startup-loader-entry-regression.log`、`startup-native-math-regression.log` |
| 浏览器 ABI 与 Firefox GLX 能力探测 | 均通过；后者取得实际 GLX 信息 | `startup-browser-regression/results.json` |
| 缓存损坏和缓存目录不可用 | 仍成功启动 Java；损坏缓存重新生成并校验头部 | `startup-cache-validation/summary.json` |
| 管理进程回收 | 退出后 processes、objects、transactions 均为 0 | `startup-manager-smoke.log` |

全仓检查不是全绿。未修改的 X11 测试存在属性快照、SHAPE 声明断言失败，随后窗口名称测试触发 BadWindow 退出；两个断言各自在独立进程中仍失败。VFS 的 `abandoned_transaction_keeps_other_process_locks_intact` 在整组测试中失败，独立运行通过，保留为测试顺序/状态问题。最初将标准输出重定向成文件还触发了 seek 测试的管道假设；改用管道后该项通过，完整 VFS 结果为 775 通过、1 失败、15 忽略。相关日志为 `startup-final-build-tests.log`、`startup-x11-isolated-*.log`、`startup-vfs-pipe-regression.log` 和 `startup-vfs-isolated-*.log`。这些检查没有被改成假成功，也没有通过跳过断言来改变生产行为。

浏览器页面、Minecraft 主菜单和所有 JNI/GUI 场景不在本轮成功结论内；已有功能边界继续见 [浏览器启动记录](browser-startup-2026-09-22.md)。

## 复现当前构建

```powershell
./tools/build.ps1 -Release -SkipFormat -SkipTests -DistDirectory artifacts/startup-optimized-dist
python tools/benchmark-startup.py --root artifacts/guest-root --dist optimized=artifacts/startup-optimized-dist --output artifacts/my-java-startup --repeat 7 --expect "openjdk version" -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version
python tools/benchmark-startup.py --root artifacts/gnome-startup-root --dist optimized=artifacts/startup-optimized-dist --output artifacts/my-firefox-startup --repeat 5 --expect "Mozilla Firefox" -- /opt/firefox/firefox --version
```

第一行是构建/打包入口，不代替功能测试。工具允许重复 `--dist 标签=目录` 做交替对照；`--profile` 开启阶段诊断，应使用独立输出目录并与正式计时分开。更改源码或客体输入后需重新验收，不能沿用本表的通过状态和数值。
