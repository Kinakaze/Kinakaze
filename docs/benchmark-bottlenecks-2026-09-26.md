# 当前项目 benchmark 与瓶颈分析（2026-09-26）

实测最值得先处理的是 **fork/exec 链路反复启动 worker、发现并绑定整套原生模块**，其次是 **文件操作每次调用的固定成本**。分配器在大量同尺寸对象同时存活时也有明显扩展性问题。小程序的 ELF 哈希、重定位不是本次首要热点。

**测试环境与范围**

- Windows 11 10.0.26200，Intel i7-13700H，20 个逻辑 CPU。
- 工作区基于 `3ae636d867120a964b64589e393f1fdfb5091ac4`，包含未提交修改。为避开并行编辑，12:27:55 将源码冻结到 `artifacts/benchmark-20260926/source`，使用该快照构建独立 Release 包。
- `build-frozen.log` 记录构建；仅在快照内刷新 `libs/libc/exports.def`。原工作区实现文件未由本次测试修改。`frozen-source-hashes.json` 与 `built-source-hashes.json` 分别记录刷新前后源码哈希，启动报告记录发行包 SHA-256。
- 测试包为 `artifacts/benchmark-20260926/dist`，客体文件树为既有 `artifacts/debian-standard-ready/rootfs`。专用文件测试只写入其中的 `/tmp/benchmark-20260926`，数据文件在测试结束时删除。
- 启动测试使用项目已有 `tools/benchmark-startup.py`：每项预热 2 次、计时 10 次，检查退出码与 `BENCH_OK`。5 项共 **50/50** 正式运行通过。
- `tools/pool-startup.py` 使用容量 2 的通用池，每项 12 次；3 个负载各测连续请求和间隔 250ms 两种方式，**72/72** 通过。准备时间单列，不计入请求时间。
- `kinakaze-alloc/examples/alloc_bench.rs` 测 1/2/4/8 线程，交替线程顺序，预热一轮后每组合 5 次；**60/60** 正式微基准记录通过。
- 文件测试每项一轮预热、5 次计时，运行 3 个独立进程，共每项 15 个正式样本。检查文件大小、读取长度和数据；文件生成和应用启动不计入计时。
- 所有性能基线关闭 profiling；分项日志、Python importtime、fork 日志和进程观察单独运行。各场景串行执行，测试进程由私有 Windows Job 管理和清理。
- 这是热文件系统/热执行缓存测量，未清系统缓存，也未测 GUI、网络、Java 或长时间服务吞吐。宿主有其他活动，启动/池各批次平均 CPU 忙碌度约 **22%–49%**；尾部和跨批次差异须保留这一限制。没有将 wall time 减去 CPU time 解释成磁盘等待。

**完整启动结果**

计时覆盖宿主启动 supervisor、init、客体 worker、应用执行和退出。P90 使用 10 个正式样本的最近秩分位数。

| 场景 | 中位数 | P90 | 最小–最大 | 进程树 CPU 中位数 |
| --- | ---: | ---: | ---: | ---: |
| BusyBox `echo` | 83.94ms | 88.90ms | 81.20–88.98ms | 78.13ms |
| Bash 内建 `echo` | 90.63ms | 102.55ms | 87.09–108.58ms | 93.75ms |
| Python 3.11 `-S` 最小程序 | 123.34ms | 125.63ms | 119.02–132.48ms | 109.38ms |
| Python 导入 ssl/sqlite3/json/subprocess | 318.69ms | 353.17ms | 285.01–373.65ms | 304.69ms |
| Bash 顺序执行 10 次 `/bin/busybox true` | 2006.52ms | 2830.15ms | 1523.96–2948.86ms | 2000.00ms |

CPU 是 Windows Job 中所有进程/线程的累计值，计数较粗，可能超过墙钟时间。Bash 外部命令场景相对单纯内建命令增加约 1.92 秒，相当于每个外部命令增加约 192ms；这是两个样本组中位数之差，不是逐次调用的独立计时。

**瓶颈一：整套 provider 初始化反复发生，fork/exec 还创建隐藏控制台**

对 BusyBox 单独 profiling 的 3 次诊断，supervisor 内部总时间中位数 86.86ms，其中：

| 分项 | 中位数 |
| --- | ---: |
| provider 发现、加载、绑定合计 | 45.40ms |
| 其中 provider 发现 | 20.45ms |
| 其中 provider 绑定 | 20.67ms |
| init 建立及连接 | 9.17ms |
| 客体 bootstrap | 6.98ms |
| 客体进程创建 | 4.10ms |
| ELF link | 2.01ms |
| 执行准备内的内容哈希 | 0.27ms |

这些计时存在嵌套，不能把表中所有行相加。诊断本身的外部总时间中位数为 94.08ms，高于未开 profiling 的 83.94ms。

代码对应：`crates/bridge/src/native.rs:309` 的 `discover` 遍历原生模块目录、解析 PE 导出、组织符号与版本，部分数据符号查询还会打开库；`crates/loader/src/providers.rs:116` 的 `load` 加载并绑定整个模块集合。本包发布 29 个原生模块。

Bash 的 10 次外部命令诊断进一步放大了这一成本：

- 三次分项运行都出现 **21 次 provider 初始化**，按单次运行累计的中位数为 **829.28ms**；其中发现 404.20ms、绑定 381.08ms。
- 另一次带进程观察的运行，Job 累计 **43 个宿主进程**。实际观察到 **22 个 worker.exe、1 个 init.exe、20 个 conhost.exe**。22 个 worker 包含 supervisor、初始 Bash worker 和 fork/exec 产生的 20 个 worker。
- 同一次运行的 10 个 fork，单次总耗时中位数 **64.30ms**；等待子 worker 就绪 **45.93ms**，CreateProcess **5.91ms**，映射复制 **6.98ms**，arena 复制 **1.38ms**。
- 复制统计中的映射数据量中位数约 **9.36MiB**，arena 使用约 2.31MiB；并非把预留的 16GiB arena 全部复制。`prepare` 含子进程创建和就绪等待，不能再与这些子项重复相加。

这组小 shell 子进程的主要问题是 worker 建立与重复初始化；仅优化内存复制很难解决全部成本。隐藏控制台确实存在，但本次没有隔离测出每个 conhost 的独立耗时。快照 `engine/crates/kinakaze-runtime/src/lib.rs:6788` 的 fork 创建路径明确使用 `CREATE_NO_WINDOW`。

建议优先检查：无控制台且 stdio 已重定向时的 detached 创建方式；已验证且不可变的模块元数据能否复用；fork 子进程和 exec replacement 是否都必须完成当前整套发现、装载与绑定。涉及 fork/exec 的改动仍需保持身份、文件描述符、信号和继承语义。

**预热池能移走固定开销，但补池会影响尾部**

| 场景 | 每次新建完整运行域 | 预热池、间隔 250ms | 预热池、连续请求 |
| --- | ---: | ---: | ---: |
| BusyBox | 83.94ms | 17.16ms | 14.33ms |
| 最小 Python | 123.34ms | 48.60ms | 71.59ms |
| Python 多模块导入 | 318.69ms | 234.64ms | 243.83ms |

表内池数据为请求到退出中位数，包含预订等待。各组池准备另耗 **66.39–131.27ms**，不能当成免费工作。连续 BusyBox 的最大请求延迟为 85.90ms，其中一次预订等待 72.32ms；连续最小 Python 最大延迟 159.70ms，最大预订等待 93.66ms。间隔 250ms 时，两者最大延迟分别为 21.84ms、55.68ms。本批每组仅 12 次，脚本给出的 P95 等于最大值，不能视为稳定尾延迟估计。

Python 导入场景即使在池中仍约 235ms。单独 importtime 诊断中，ssl 累计 165.05ms、sqlite3 27.59ms、json 16.19ms、subprocess 34.40ms，包含各自依赖和诊断开销。装载诊断的 ELF link 累计约 28.60ms、内容哈希累计约 3.71ms，说明只优化哈希无法消除应用导入剩余成本；目前没有把全部导入时间归因到单一 VFS 函数。

**瓶颈二：文件操作的固定成本，尤其是小块 read**

以下为三个独立运行合并后的 15 个样本中位数。客体为 Python 3.11.2；宿主参照为 Windows Python 3.13.12，使用相同脚本和同一宿主目录。解释器、CRT 和文件语义不同，因此宿主数字只作定位参照，不能称为同版本 Linux 原生性能比。

| 操作 | 客体 | 宿主参照 |
| --- | ---: | ---: |
| 1000 次 stat | 138.23ms | 30.80ms |
| 1000 次 fstat（复用 fd） | 63.93ms | 5.02ms |
| 1000 次 open + close | 480.11ms | 33.91ms |
| 8MiB，4KiB 分块，2048 次 read | 185.73ms | 8.10ms |
| 8MiB，64KiB 分块，128 次 read | 11.49ms | 4.74ms |
| 8MiB，1MiB 分块，8 次 read | 2.68ms | 6.34ms |

同一客体读相同数据，分块从 4KiB 到 64KiB，调用数减少 16 倍、合并中位耗时约减少 16 倍。这支持“每次调用固定开销很大”的判断，不能据此说磁盘带宽是瓶颈。小块 read 的单批中位数为 342.91/186.72/123.20ms；open+close 为 1846.76/480.11/435.36ms，抖动明显，全部样本均保留，没有剔除较慢批次。

代码中可明确看到以下额外工作，但本次没有分别测出每一步占比：

- `engine/crates/kinakaze-vfs/src/path.rs:383`：路径逐组件解析，并逐项检查模拟符号链接。
- `engine/crates/kinakaze-vfs/src/fs.rs:2022`：`stat_handle` 重新打开元数据查询句柄；`stat_with_query` 查询文件信息、inode 扩展属性、权限以及 verity 逻辑大小。因此 fstat 本身仍有成本，不能把问题全部归结为路径解析。
- `engine/crates/kinakaze-vfs/src/lib.rs:3998`：普通文件 read 进入 `verity::verified_read`。
- `engine/crates/kinakaze-vfs/src/fs/verity.rs:425`：检查访问权、重新打开数据/EA 句柄；`:451` 的 `read_object` 获取 inode 锁、读取 verity 元数据和大小后才读数据，即便文件没有启用 verity。
- `engine/crates/kinakaze-vfs/src/xattr.rs:88`：inode 锁每次创建/打开命名 mutex 并等待。

优化方向：先量化普通、未启用 verity 文件每次 read 的句柄重开、EA 查询与锁成本，评估在保留跨进程一致性、取消隔离及 verity 状态切换语义下复用句柄和状态；应用侧增加缓冲区也能直接减少调用次数。元数据路径应减少重复查询，并明确失效条件。

**瓶颈三：同尺寸对象大量存活时，分配器并发扩展下降**

原有微基准的单位为每秒完成的 malloc/free 对，总工作量随线程数增加。

| 场景 | 1 线程 | 2 线程 | 4 线程 | 8 线程 |
| --- | ---: | ---: | ---: | ---: |
| small，13 种 16–1024B 尺寸 | 27.67M/s | 48.43M/s | 92.78M/s | 145.85M/s |
| mixed，16B–64KiB | 24.93M/s | 34.63M/s | 48.49M/s | 73.76M/s |
| bulk，每批 1024 个 128B 对象 | 25.35M/s | 31.05M/s | 41.03M/s | 34.76M/s |

small 的 8 线程吞吐约为单线程 5.27 倍；bulk 从 4 到 8 线程反而下降约 15%。这不是所有分配场景都同样慢。

追加等工作量对照：每个线程固定 **512,000 对**分配/释放，只将 bulk 从 `1024 × 500` 改成 `64 × 8000`。使用同一个 allocator DLL，两个驱动均 opt-level=3/codegen-units=1，交替顺序，每组合预热后 5 次：

| 线程数 | 每批 64 个 | 每批 1024 个 |
| --- | ---: | ---: |
| 1 | 36.05M/s | 33.86M/s |
| 4 | 75.56M/s | 58.04M/s |
| 8 | 84.57M/s | 47.44M/s |

8 线程下每批 64 个吞吐是每批 1024 个的 **1.78 倍**。两种批量同时也改变了活跃对象数量和工作集，所以这不是锁等待占比的直接测量。

对应 `engine/crates/kinakaze-alloc/src/lib.rs:116` 的每个尺寸类本地缓存上限 64，`:1128` 的 miss 批量从中央链表补充、`:1214` 的满缓存向中央链表归还、`:1627` 的自旋及 WaitOnAddress。实测支持优先调查高批量下的中央 bin 交换与争用；缺少采样栈，不能声称所有下降都来自同一把锁。宿主混合核心与调度也会影响线程扩展。

建议针对高频尺寸类统计 refill/spill 与争用次数，再评估中央链表分片或自适应缓存容量；不宜仅凭本次微基准统一放大全部线程/尺寸类缓存，需同时评估常驻内存和 fork 成本。

**优先级与复现**

1. CLI/脚本场景先处理重复 provider 初始化，以及 fork/exec 路径的隐藏控制台；这里已有完整运行时间、分项日志和进程数量相互印证。
2. 文件密集场景先处理每次 read 与元数据查询的宿主调用成本；在函数分项证据补齐前，不将其误判为物理磁盘吞吐问题。
3. 高并发/大批量分配场景调查中央空闲链表压力；普通小对象本地缓存路径已有较好的扩展。
4. 池化场景检查补池速度和突发容量；记录准备成本、等待和完整退出时间。

本次只增加报告和测试产物，没有修改产品实现，也没有把性能探针通过当作完整兼容性回归。正式构建使用 `-SkipTests`；上述退出码、输出及数据正确性检查均实际执行。

在仓库根目录使用已经生成的冻结包重跑：

```powershell
python artifacts/benchmark-20260926/run_suite.py startup
python artifacts/benchmark-20260926/run_suite.py io --tag=-new
python artifacts/benchmark-20260926/run_suite.py alloc
python artifacts/benchmark-20260926/alloc_batch_compare.py
python artifacts/benchmark-20260926/diagnose.py
python artifacts/benchmark-20260926/analyze_results.py
```

`run_suite.py` 复用仓库原有启动和池脚本。`io_workload.py` 为本次新增的可检查文件操作微基准；`alloc_bench_batch64.rs` 为等工作量对照驱动。等工作量驱动的编译命令为：

```powershell
rustc --edition=2024 -C opt-level=3 -C codegen-units=1 -C prefer-dynamic -C linker=lld-link artifacts/benchmark-20260926/alloc_bench_batch64.rs --extern kinakaze_alloc=artifacts/benchmark-20260926/alloc-build/release/deps/kinakaze_alloc.dll -L dependency=artifacts/benchmark-20260926/alloc-build/release/deps -o artifacts/benchmark-20260926/alloc-build/release/examples/alloc_bench_batch64.exe
```

原始结果全部位于 `artifacts/benchmark-20260926/`：

- `startup-*/results.json`、`pool-*/results.json`：逐次结果、命令、包哈希；各目录另有输出与宿主负载。
- `profile-*/current-*.profile/`：3 次独立启动分项日志。
- `analysis.json`：启动、池及分项汇总；嵌套阶段不应相加。
- `fork-diagnostic/`：fork/交接计时、所观察到的宿主进程和完整输出。
- `python-importtime/`：Python 各模块导入诊断。
- `io-guest*.stdout.log`、`io-host*.stdout.log`、`io-fork-analysis.json`：文件测试全部样本和汇总。
- `allocator-results.json`、`allocator-batch-results.json`：线程扩展与等工作量对照，后者包含驱动与 allocator 哈希。
- `source/`、`dist/`、源码哈希及构建日志：本次实际构建的快照和二进制；报告代码行号以冻结快照为准。
