# 第五轮：复用不可变映像 section，减少 Java 启动复制

已直接修改底层 ELF 装载：布局适合时，执行映像从已验证的不可变快照创建私有写时复制视图，省去重复初始化一个完整 section。最终测试包为 `artifacts/startup5-final-dist`。本轮继续保留通用 init 预热池；并行哈希试验没有证明完整启动收益，已撤回默认实现。

200ms 的完整目标尚未完成：最终包的 20 次预热 `java -version` 全部低于 200ms，但完整 Java JIT/线程/文件工作负载仍超过 200ms。现有 Minecraft 26.2 客户端在普通启动和通用池激活后均未在 25 秒内提交首帧，不能把版本输出或 main 入口当成大型应用就绪。

## 实现与功能边界

原路径读取不可变文件快照，然后再把 PT_LOAD 数据复制到另一个 section，最后建立执行用 COW 视图。新路径仍完整读取和验证文件，但允许快照底层 section 支持额外的执行用 COW 视图；源视图发布后保持只读、不可执行。

只有满足以下条件时才复用：

- 快照 section 足够覆盖整个虚拟映像，所有源/目标边界有效。
- PT_LOAD 内存范围互不重叠；重叠或其他特殊布局继续走原复制路径。
- 至少 1MiB 文件数据已经位于目标偏移，可直接复用。
- 需要处理的其余字节不超过可直接复用字节的四分之一，避免将巨大 BSS 或稀疏区域提前变成私有实页。

偏移不同的段仍复制；段间空隙、BSS 和 PT_LOAD 以外的映射字节按原路径补零。所有改写发生在新的私有 COW 视图中，不修改源快照。执行视图拥有单独复制的 section 句柄，保留 `CopyOnWriteSection`、`GuestMm`、`PAGE_EXECUTE_WRITECOPY` 和原 fork 注册逻辑。源快照先释放时执行映像仍可使用。数据用途的普通不可变快照保持原权限；执行视图创建不受支持时回退到原有独立 section 路径。

Windows 的 [MapViewOfFile 文档](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-mapviewoffile)说明了 COW 写入私有页及执行映射的权限要求。没有把共享映射改成共享可写，也没有跳过 ELF/AOT/文件内容验证。

对本机 `libjvm.so`，可复用部分为 23,661,761 字节（约 22.57MiB），整个执行映像映射为 25,317,376 字节，其余约 1.58MiB 按原语义处理。剖析观察到映射约 1.41–1.56ms，其中局部处理约 1.02–1.12ms；此前大段复制本身约 11–15ms。快照读取仍有约 16–18ms，执行准备仍有约 17–19ms，这些工作并未消失。不同时间的剖析只用于定位分项，完整收益采用下面的配对结果。

## 受控对照与最终包结果

20 组交替顺序配对，同一冻结代码背景、同一 Linux Temurin 25.0.4.1+1、同一 JavaRuntimeProbe。两组各维持一个容量 2 的通用池，每次完整退出后间隔 0.5 秒；关闭 profiling。请求计时包含预订、激活和原生退出确认，准备及间隔不算进每次请求，均在原始记录中明确。文件系统和 AOT 缓存为热状态。

| 指标 | 原路径 | 共享快照执行视图 |
|---|---:|---:|
| 请求至 main 中位数 | 117.55ms | 110.57ms |
| 请求至 main P95 / 最大 | 130.05 / 131.62ms | 141.21 / 153.26ms |
| 完整 JIT/线程/文件测试至退出中位数 | 233.22ms | 223.53ms |
| 完整退出 P95 / 最大 | 261.79 / 279.04ms | 273.17 / 291.80ms |
| 独立应用进程缺页中位数 | 31,553.5 | 25,515 |
| 独立应用进程累计 CPU 中位数 | 312.50ms | 343.75ms |

main 和完整退出的中位数分别减少约 5.9%、4.2%，缺页减少约 19.1%。**尾部没有同步改善，累计 CPU 也没有证明下降。** CPU 时间粗粒度、多线程累计、JIT 的异步工作以及宿主竞争都会影响统计；测得应用周期数中位数约 989M / 983M，不能据此宣称显著降低 CPU 总成本。样本间宿主 CPU 观察值约 25%–66%，没有终止其他任务来制造空闲测试条件。

配对后增加了稀疏映像/BSS 的保守回退条件；上述 libjvm 布局仍走相同快路径。最终包单独做 20 次 `java -version`：

| 指标 | 结果 |
|---|---:|
| 请求至退出中位数 | 129.62ms |
| P95 / 最大 | 152.57 / 157.72ms |
| 低于 200ms | 20 / 20 |
| 该批容量 2 的池首次准备 | 323.08ms |

该独立批次不能与上一轮 190ms 中位数直接相减并全部归因于映射改动。它只证明这批预热版本测试达到了 200ms；首次建池、更复杂工作负载、冷缓存和大型应用就绪仍是独立口径。

## SIMD、CPU 与未保留的并行哈希试验

先测试了最多 4 个短生命周期线程执行标准 BLAKE3 子树计算，结束前全部回收，保持完整内容哈希和同一缓存键。独立约 30MB 输入微基准从 8.48ms 降至 3.05ms，并覆盖 chunk/子树边界与标准哈希一致性。

但实际应用中线程准备和调度也有成本。20 组真实配对的完整退出中位数为 289.53ms / 291.86ms，P95 为 406.95ms / 523.70ms，没有证明总体收益。因此该实验代码只保留在 artifacts，源码恢复已有单线程 SIMD BLAKE3。没有通过减弱校验或挑选微基准宣称启动优化成功。

JIT/线程仍按应用正常执行。映射优化减少了复制和缺页，但不会替应用完成类初始化、数据准备、资源加载和图形初始化。CPU/等待证据仍以进程累计计数、线程周期、阶段墙钟和真实就绪观察为限；没有新增可用的 ETW/off-CPU 栈，不能把未解释耗时直接称为磁盘等待。

## 大型 Java 应用的边界检查

使用仓库已安装、校验符合元数据的 Minecraft 26.2 客户端与现有 Linux JRE，demo 参数，单独的游戏和 native 提取目录；不使用账号凭证或已有世界。仅观察该测试私有 Job 内的窗口与首帧标记。

- 原包普通启动：25.03 秒观察超时，没有首帧。
- 候选包通用池：准备约 182.86ms，激活后观察 25.01 秒超时，没有首帧。
- 两次都出现应用 Datafixer 初始化和系统信息探测；普通启动日志中 Datafixer 自报约 2.1 秒，并出现文件系统容量查询 `Invalid argument`、缺少 `/etc/os-release` 等警告。

这些日志不足以把全部等待归因于某个警告。运行与测试编译有重叠，因此不作为两包速度优劣的比较；它们确实没有达到实际首帧验收。测试均已清理自己创建的进程。需要继续对应用初始化和兼容性路径做分项定位；main 小探针的结果不能替代它。

## 验证与复现

最终包对应的相关 Rust 测试：283 通过、0 失败，5 个既有忽略/helper 项。新增映射测试逐字节比较原复制结果，覆盖不同段偏移、间隙、BSS、只读源不受私有写影响、源先释放、固定地址、数据 section 回退、失败清理及稀疏布局回退。已有内核 fork/COW/TLS/exec 测试和 6 项映像 IO 完整性/替换/unlink 测试通过。

最终包的真实通用池生命周期、Java JIT/线程/文件/8 次 spawn、Node/Python 行为探针通过。普通源快照、执行视图和 fork/exec 的所有权机制保留，没有复用已经执行过应用的 worker。

```powershell
python tools/pool-startup.py --root artifacts/guest-root --dist artifacts/startup5-final-dist --output artifacts/my-java-pool --repeat 20 --interval 0.5 --expect 'openjdk version' -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version
python tools/test-init-pool.py --root artifacts/guest-root --dist artifacts/startup5-final-dist --output artifacts/my-pool-regression
```

原始记录：

- `artifacts/startup5-section-paired/results.json`：20 组完整配对、宿主负载、分发包 SHA-256。
- `artifacts/startup5-final-java-version-20/results.json`：最终包的 20 次版本启动。
- `artifacts/startup5-section-profile`：实际 libjvm 快照、映射、准备与 JVM 日志。
- `artifacts/startup5-final-tests`、`startup5-final-lifecycle`、`startup5-pool-node-python`：最终相关功能回归。
- `artifacts/startup5-hash-paired`、`startup5-hash-probe`：未保留方案的完整结果和实验代码。
- `artifacts/startup5-minecraft-baseline.json`、`startup5-minecraft-pool.json` 及对应日志：大型应用首帧未通过记录。
- `artifacts/startup5-source-manifest.json`：本轮负责文件与测试快照的内容哈希。

继续使用 `artifacts/startup-source` 固定背景构建，避免其他任务不断变更工作区影响归因。活动工作区的 AOT 控制流版本等其他改动均保留，未回退到冻结背景；这些不同背景的完整兼容性不由本报告代为证明。目标仍为实际应用的可靠就绪时间，尚未标记完成。
