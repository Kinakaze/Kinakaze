# Java 启动继续优化：完整校验与按需绑定保持不变

本轮已经降低启动耗时，但**尚未达到接近本机 1×**。最终同批交替测量中，最小 Java 程序进入 main 的中位数从 161.709ms 降到 154.352ms（4.55%）；本机为 58.176ms，新版仍为 2.65×。不能把局部哈希的加速当成整体接近原生。

## 同批本机对照

原始数据：`artifacts/java-startup-20260926/native-final/results.json`。

每种运行方式、每个用例 30 次正式运行、3 次预热，共 270 次正式运行，全部通过。三种方式轮换、反转顺序；每次新建进程及 JVM，使用同一份 class 字节码。main 时间为宿主启动到读到首个 main 标记，exit 时间为宿主启动到进程结束；OS 文件缓存已热。测试没有预启动 JVM、复用 init 池或提前绑定来宾符号。

| 用例与口径 | 本机 Windows | 上轮版本 | 本轮版本 | 相对上轮 |
|---|---:|---:|---:|---:|
| 最小程序：进入 main | 58.176ms | 161.709ms | 154.352ms | -4.55% |
| 最小程序：退出 | 81.589ms | 185.989ms | 176.652ms | -5.02% |
| `java -version`：退出 | 78.809ms | 192.736ms | 174.683ms | -9.37% |
| JIT/线程/文件探针：进入 main | 62.547ms | 167.306ms | 156.332ms | -6.56% |
| JIT/线程/文件探针：退出 | 168.731ms | 286.125ms | 273.951ms | -4.25% |

最小 main 的 P95：本机 72.225ms，上轮 188.610ms，本轮 169.717ms。本轮三个用例的宿主平均 CPU 忙碌比例为 29.8%–33.3%；早先 `native-gate`、`native-hash` 和 `native-prefault` 的波动记录一并保留，尤其 `native-prefault` 的功能探针遇到明显调度停顿，不使用它作为最终数据。

本机是已安装的 Oracle HotSpot 23.0.1，来宾是 Temurin 25.0.4.1。版本不同，因此这是实际本机比较，不能将全部差距归因于兼容层。前一份报告的 41.6ms/114.2ms 来自更早的负载窗口，不能跨批次直接比较绝对耗时；本轮上轮/新版/本机均在同一批测量。

## 实际修改

1. **并行启动管理端和工作进程。** 启动 init 后立即创建 worker，让 Windows 进程初始化及运行库加载与 init 就绪过程重叠。随机命名、仅当前用户可访问的事件在 READY 和控制端连接成功后放行 runtime session。事件不继承；worker 在加载运行库前消费并清除环境变量。失败路径仍回收 worker 和 init，exec 后仍等待逻辑 PID 的最终状态。
2. **大型 ELF 的完整 BLAKE3 并行计算。** 8MiB 以下保持原路径；大型文件按 BLAKE3 合法子树边界最多使用 4 个线程。摘要与原算法逐字节相同，原 AOT 缓存键保持兼容。线程创建失败退回串行计算，不缩减校验范围。临时线程全部 join 后释放 fork 映射事务，没有遗留全局线程池。
3. **大型可执行快照并行准备页面。** 16MiB 以上的匿名 section 在读取前用最多 4 个线程触及互不重叠的页，降低 ReadFile 串行处理新页的开销。仍通过原来的逻辑 EOF/verity 读取器填充完整内容，再发布只读快照。重命名、unlink、COW、fork 和失败清理语义保留。

第三项只改变页面驻留时机；第二项缓存的仍是指令修补位置，没有保存已绑定符号地址。现有依赖解析、首次 dlsym、版本校验和 COPY 重定位路径保持不变。

并行优化会增加短时 CPU 并行度。最小程序的 Job CPU 计时中位数由 195.312ms 增到 234.375ms；该计数粒度较粗，不代表节能优化，收益在于缩短关键路径的墙钟时间。

## 剖析与剩余开销

独立开启 profiler，旧/新版各 7 次；这些带日志运行不混入上表。中位数如下，阶段存在包含关系，不能直接求和。

| libjvm.so 阶段 | 上轮 | 本轮 |
|---|---:|---:|
| 29,876,336 字节只读快照 | 14.534ms | 11.912ms |
| 完整内容哈希 | 8.501ms | 3.411ms |
| 修补缓存校验 | 1.426ms | 1.501ms |
| 指令修补应用 | 6.504ms | 6.814ms |
| 整体代码准备，包含哈希/校验/修补 | 18.012ms | 13.385ms |

普通分块读取、额外中转拷贝和串行触页的微基准没有稳定收益，未进入生产代码。并行哈希独立微基准为 6.711ms → 2.276ms；该数字仅解释机制，整体收益以上表为准。

`cds-valid` 和 `native-cds.log` 确认双方均在使用 CDS，来宾类共享没有失效。剩余差距涉及 ELF 快照/映射、指令翻译、运行库及管理进程启动，以及不同 JDK 的初始化行为。本轮保留完整检查和新 JVM 的测量口径，没有通过关闭校验、改变 JVM 默认参数、跳过功能或预绑定来凑 1×。

## 验证

- 98 项相关测试通过，3 项原有测试忽略：host-win 10、execution 37、immutable 3、image_io 6、link 42。覆盖新哈希在块/子树边界及不均匀尾部与标准 BLAKE3 相等；大型可执行快照的零初始化、只读发布和 COW；原有完整性、文件共享、链接器和指令语义。
- 当前工作区 `cargo check --workspace --locked --features kinakaze-v2-runtime/guest-engine` 通过。
- 其他启动用例各版本各 15 次、共 120 次正式运行全部通过。中位数：BusyBox 68.934→68.000ms，Bash 内建命令 82.487→76.599ms，Python 导入 278.577→267.978ms，Bash 启动 10 个子进程 984.738→951.950ms。原始记录位于 `paired-tools/`。
- 标准 ABI、loader entry、环境/fork、日常命令、真实 Python、共享 fork I/O、PTY/进程启动和 init 池的回归通过；init 池包含 Java ProcessBuilder 等 16 个场景。
- 新增 `tests/guest/LargeImageProbe.c` 和 `.py`：反复按需加载 16MiB ELF，同时从另一线程 fork，检查首部、中部、尾部和零区字节。两批正式运行共 12 次通过。负载稳定的复测中，本轮中位数 957ms，上轮 1017ms；较早受干扰的一批也保留。
- 延迟绑定跟踪通过：RTLD_NOLOAD 不实例化模块，首次显式查找前不解析来宾符号，fork 后按需恢复；未使用模块的文件视图仍保持不可写。
- 模拟 init 在 READY 前失败：supervisor 返回错误，来宾代码没有执行；关闭测试 Job **之前**活动进程数已为 0。

## 产物与复测

本轮目录：`artifacts/java-near-native-20260926/`。

- `final-dist/`：已验证候选版本的逐文件副本；默认 `dist/` 未覆盖。
- `source/`、`before/`、`changes.patch`、`final-source.json`：固定构建源、修改前副本、本轮补丁，以及 11 个改动源码与工作区一致的哈希。
- `profile-final/phase-summary.json`、`regressions/`、`demand-verification-candidate/`、`large-image-fork-repeat/`、`gate-failure.json`：剖析及回归证据。
- `hash-dist/`、`gate-dist/`：中间版本，便于进一步做拆分对照。

```powershell
python artifacts/java-startup-20260926/compare_native.py --repeat 30 --warmup 3 --tag native-recheck `
  --dist F:/crysoacu2/artifacts/java-near-native-20260926/final-dist `
  --previous F:/crysoacu2/artifacts/startup-extreme4-20260926/final-dist

python artifacts/java-near-native-20260926/run_regressions.py final
python artifacts/java-near-native-20260926/verify_deferred.py final
```

大型并发加载探针先用 `clang --target=x86_64-linux-gnu -fuse-ld=lld -shared -fPIC -O2 -nostdlib '-Wl,--hash-style=both'` 编译 `.c` 到来宾根目录，再用来宾 Python 运行 `.py`，参数为该 `.so` 的来宾绝对路径。运行时应使用 `tools/session_process.py` 的 Job 所有权或 `tools/benchmark-startup.py` 的超时清理。
