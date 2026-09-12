# 第四轮：通用 init 预热池与 Java 启动瓶颈

已实现按会话维持、自动补充的通用预热池。init 准备 worker 时不需要知道应用命令；准备完成后，控制方才指定程序、参数、工作目录和环境。每次使用一个未运行过应用的新 worker，可同时运行多个应用。

本轮 Java 到达 `main` 的 20 次采样全部低于 200ms，中位数 130.26ms、P95 146.77ms、最大 168.87ms。但完整 JIT/线程/文件测试至退出中位数为 250.02ms；`java -version` 的 P95 仍有 230.88ms。**尚未达到所有启动稳定低于 200ms，更没有以 main 入口替代大型服务或 GUI 可用性。** 原有普通启动、固定命令预热入口均保留。

## 通用池的实现

- init 参数：`--prewarm-root ROOT --prewarm-dist DIST --prewarm-pool N`，N 为 1–8，初始阶段不传应用命令。
- worker 预先打开 runtime、认证会话、加载和绑定 native providers，然后阻塞等待。应用 ELF、IFUNC、构造函数及 JVM 均在激活后才执行。
- 协议依次使用 `AwaitPoolReady`、`ReservePoolWorker`、`ActivatePoolWorker`、`AwaitExit`。保留容量不足时的真实排队；预订只属于该控制连接，显式释放或连接 EOF 会归还尚未激活的 worker。
- 一个 worker 只接受一次应用配置。同配置重放幂等，不同配置重放被拒绝。应用运行后的进程不再回池；fork/exec 子进程不会变成待用 worker。
- 平时在激活后等待 250ms 再补池，以减少争用；已有请求排队时立即补池。连续请求不会反复延后最早的补池期限。
- 准备阶段使用较低进程优先级，发布就绪前恢复普通子进程应有的优先级。满池空闲时使用条件变量阻塞，不轮询。准备超时或重复失败会让等待方收到错误。
- 创建 replacement 和加入 init Job 的操作与 shutdown 共用互斥锁；原生 PID 配合创建时间固定进程身份。init 退出或控制进程死亡由 Job 清理会话，避免 PID 重用及补池/关闭竞态留下孤儿进程。

会话固定 root 和 distribution，应用命令可任意选择。默认池容量指尚未使用的 worker 数量；已激活应用可以继续存活，容量不是总运行应用数限制。标准输入为 EOF，标准输出/错误属于会话日志，当前接口不是交互终端或逐应用独立标准流接口。

## 底层映射修改与测量

`ImmutableBytes::initialize` 以前填充可写 section 视图后先取消映射，再创建只读视图。现在在同一已填充视图上调用 `VirtualProtect(PAGE_READONLY)`，保持驻留页表项，减少后续解析、哈希、复制再次触发软缺页。仍然保留同一不可变 section、只读保护、fork 注册及所有权清理，没有暴露可写别名。

Windows 要求新保护权限与映射权限兼容，见 [VirtualProtect 文档](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualprotect)。实际测试检查了只读权限、8MB 内容、句柄释放、初始化失败清理及 fork 映射。ELF 执行映像继续使用独立的私有写时复制视图；它的语义不同于只读快照，见 [MapViewOfFile 文档](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-mapviewoffile)。进一步尝试将 COPY 视图原地转为共享写入的隔离实验返回 Windows 错误 87，没有将这条不可用路径写入生产实现。

在同一冻结代码背景下、只改变不可变视图实现的 10 组交替配对测试：

| 指标 | 修改前 | 修改后 |
|---|---:|---:|
| Java main 入口中位数 | 167.40ms | 157.26ms |
| 完整 JIT/线程/文件测试至退出中位数 | 319.93ms | 306.28ms |
| Job 缺页中位数 | 43,120 | 35,250.5 |
| main 最大值 | 212.35ms | 392.44ms |

该早期配对中位数分别改善约 6.1%、4.3%。不能省略候选版的较慢尾部，也不能把不同时间批次间的额外改善全部归因于映射修改。完整测试期间补池与应用重叠，以上 Job 缺页包含补池，不能当作应用自身精确减少量。为解决这一测量问题，最终脚本新增按原生进程身份固定句柄的单应用 CPU/缺页计数。

另做四批交替顺序 `旧、新、新、旧` 的突发请求测试，每批一个新池、容量 2、连续执行 12 个短 shell 程序。对发生明显预订等待的请求（预订大于 5ms），旧版中位数 387.15ms、最大 420.82ms；按需求立即补池后中位数 127.97ms、最大 191.75ms。两版分别有 13、15 个请求落入这个子集。全部请求中位数为 25.32ms、33.57ms；改进集中于等待补池的请求，没有宣称每一请求都更快。这是 shell 突发排队实验，不是 Java/GUI 启动结果。

## 最终 Java 结果及当前瓶颈

环境：本机 Windows、i7-13700H、Linux Temurin 25.0.4.1+1、Release 构建。使用 `startup4-release-dist`，关闭所有 profiling，池大小 2，每次完整退出后间隔 0.5 秒。没有清空文件系统缓存，执行代码缓存已存在。请求计时包括预订排队、控制 RPC、应用运行及 init 确认原生退出；常驻 init 的最终关闭不属于每请求耗时。准备成本单独列出，间隔也没有隐藏。

| 20 次测试的口径 | 中位数 ms | P95 ms | 最大 ms |
|---|---:|---:|---:|
| JavaRuntimeProbe 请求至 main 标记 | 130.26 | 146.77 | 168.87 |
| 同一程序完成 JIT、线程、文件测试并退出 | 250.02 | 290.69 | 295.61 |
| java -version 请求至退出 | 190.16 | 230.88 | 240.61 |

两批池首次准备分别为 329.31ms 和 315.22ms。main 测试 20/20 低于 200ms；版本退出测试只有 13/20 低于 200ms。main 标记由宿主读到输出时打点，包含管道传送和宿主调度延迟。此探针没有大型应用的资源加载、服务监听或 GUI 渲染，因此仍需明确实际应用的就绪点。

main 所在完整工作负载的独立应用进程 CPU 中位数为 351.56ms，其中用户态中位数 156.25ms、内核态 203.13ms，缺页中位数 31,512。各字段中位数不能直接相加，CPU 为多线程累加，可以超过 250ms 墙钟。一个准备完成的 worker 工作集约 18.27MiB；工作集含共享页，不能据此简单乘出整池独占内存。

最终包另做三次带剖析运行，定位 `libjvm.so` 的分项：

| 分项 | 观察范围 |
|---|---:|
| 读取到不可变快照（约 29.88MB） | 21.05–22.13ms |
| 复制 ELF 段到 section | 14.00–14.89ms |
| section 创建 / 发布私有视图 | 0.03–0.04ms / 约 0.02ms |
| relocation | 2.18–2.80ms |
| 执行准备（含下列 hash/cache） | 27.21–29.84ms |
| 全内容 SIMD 哈希 | 10.30–13.83ms |
| cache 验证 / 应用 | 2.41–3.37ms / 11.03–13.11ms |
| JVM 自报 Create VM | 72.14–83.29ms |

分项存在包含关系，不能重复相加。另一次较早剖析中 JVM 创建为 49–50ms，段复制约 11ms，说明运行条件存在波动；带日志的剖析不替代关闭剖析的性能表。当前主要成本是大映像快照与复制、页面建立与补丁，以及 JVM 初始化，预订 RPC 在 20 次 main 测试中位数仅约 0.54ms。

完整应用 CPU 与整个 init Job CPU 分开记录，后者还含补池及应用子进程。预热等待阶段线程计数无可测 CPU 增量，空闲池测试 Job CPU 增量为 0；粗粒度计数不意味着完全没有执行指令。缺页包含软缺页，IO 字节包含缓存 IO；无法用“墙钟减累计 CPU”计算磁盘或锁等待。此前 WPR 返回 `0xc5585011`，本轮没有声称取得 ETW/off-CPU 等待栈。对等待的判断限定于池屏障、排队时间和已有线程周期/阶段计时。

JIT 日志再次确认 `JavaRuntimeProbe::sum` 进入第 3、4 编译层。沿用第三轮确认的 CDS、TieredCompilation、AVX/SSE 路径，未关闭 JIT 或降低最终编译层级。内容哈希继续使用具备 SIMD 派发的 BLAKE3，并校验完整内容；没有以时间戳或抽样替代校验。通用池在应用未知时只能提前共用 runtime/provider 工作，不能预先执行任意应用的 Java 类初始化而仍保证没有应用副作用。

## 使用方式

从仓库根目录运行可重复测量：

```powershell
python tools/pool-startup.py --root artifacts/guest-root --dist artifacts/startup4-release-dist --output artifacts/my-pool-run --size 2 --repeat 20 --interval 0.5 --expect 'openjdk version' -- /usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java -version

python tools/test-init-pool.py --root artifacts/guest-root --dist artifacts/startup4-release-dist --output artifacts/my-pool-tests
python tools/test-prewarm.py --root artifacts/guest-root --dist artifacts/startup4-release-dist --output artifacts/my-pool-failures --pool-size 2
```

`--commands-json` 可指定多种程序及各自的 cwd/environment/expect/expected_exit/ready_marker；`--profile` 开启阶段诊断。JVM 中含冒号的参数应在 PowerShell 中加引号。

宿主代码将仓库 tools 目录加入 Python 搜索路径后，可保持同一个会话并独立等待各应用：

```python
from init_pool import InitPool

with InitPool(root, dist, output, size=2) as pool:
    first = pool.launch(['/bin/sh', '-c', 'exit 7'], cwd='/tmp')
    second = pool.launch(['/usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java', '-version'])
    assert first.wait() == 7
    assert second.wait() == 0
    pool.shutdown()
```

`InitPool` 默认没有会话总时限，适合长时间运行；测量 CLI 显式设置总时限，默认 120 秒，可用 `--timeout` 调整。退出上下文会清理会话。日志完整写盘，内存只保留有限历史；大输出不会因超过 64MiB 就终止应用。ready/expect 标记会跨读块匹配，并保持已观察结果。并发应用共享会话输出，需要使用唯一标记。

## 验证记录与范围

最终源码对应的 274 项相关 Rust 测试通过，0 失败，另有 4 个既有忽略/helper 项。包含新增 4 项池状态机测试、ELF 私有映射/保护、不可变快照、fork 映射、TLS、exec、原生生命周期和控制协议。

真实通用池验证覆盖不同命令、cwd/environment 隔离、独占预订、释放/EOF、多个应用并行、重复 wait、超容量补池、空闲 worker 崩溃、准备优先级恢复、关闭清理；额外三个关闭/补池竞态测试在关闭外层测试 Job 前确认活跃进程数为零。准备失败、取消及控制进程死亡测试通过。固定命令预热的四项生命周期回归也通过。

Java JIT/线程/文件/8 次 spawn、通用池内 Node 与 Python 的线程、压缩、文件、子进程和本地网络探针通过；普通入口的 allocator interposition/IFUNC、Python runtime 和 allocation lifecycle 通过。65MiB 输出与历史裁剪测试确认早期标记仍有效，下一应用不能复用旧标记。两次最初 Python 探针选错了不含 Python 的 guest-root，记录了文件不存在；改用 tool-root 后通过，未将错误配置的运行计作性能结果。

主要原始记录：

- `artifacts/startup4-java-main-20/results.json`、`startup4-java-version-20/results.json`：最终性能与全部分发文件 SHA-256。
- `artifacts/startup4-immutable-paired/results.json`、`startup4-refill-paired/results.json`：两项受控对照。
- `artifacts/startup4-java-profile-release`：loader 分项、线程 CPU/周期、JVM 初始化/JIT 日志。
- `artifacts/startup4-release-tests`、`startup4-lifecycle-release-races`、`startup4-pool-failures-release`：最终相关回归。
- `artifacts/startup4-fixed-prewarm-regression`、`startup4-pool-node-python`、`startup4-pool-output-regression`：入口兼容与真实运行。
- `artifacts/startup4-source-manifest.json`：本轮负责的完整文件与已测快照的内容哈希。

为了避免其他任务同时改动工作区污染比较，使用 `artifacts/startup-source` 固定背景构建，按需同步本轮修改。测试分发包为 `artifacts/startup4-release-dist`。linker/object 中仅同步本轮剖析插入，保留活动工作区其他任务的 TLS 等修改。没有覆盖它们，也没有宣称整个活动工作区或所有桌面应用均已通过验收。预热池和底层修改已直接落入源码，200ms 的整体目标继续保留。
