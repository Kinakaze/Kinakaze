# 启动性能优化验证（2026-09-26）

已落地两轮启动优化。固定相同源码背景构建 Release 基线和优化版，最终每项各 20 次交替测量，160 次正式启动全部成功。连续启动子进程收益最大；短命令仍以 provider 装载为主要开销。

| 场景 | 基线中位数 | 优化版中位数 | 耗时下降 | 基线 / 优化版 P95 |
| --- | ---: | ---: | ---: | ---: |
| BusyBox echo | 67.05 ms | 63.10 ms | 5.9% | 77.26 / 72.13 ms |
| Bash 内建 echo | 75.44 ms | 67.82 ms | 10.1% | 87.46 / 71.99 ms |
| Python 导入 ssl、sqlite3、json、subprocess | 222.46 ms | 212.98 ms | 4.3% | 242.99 / 228.09 ms |
| Bash 连续执行 10 次 BusyBox true | 1395.72 ms | 1063.36 ms | 23.8% | 1448.90 / 1128.74 ms |

统计的是请求启动到进程退出的完整耗时，包括 supervisor、init、worker 和 guest 执行。每项预热 2 次，每轮交换两个版本的执行顺序；正式测量关闭分阶段日志。P95 使用 nearest-rank，20 个样本不足以评估极端长尾。

Windows 11 / i7-13700H / 20 个逻辑 CPU，热文件缓存，测试阶段主机平均忙碌度约 15.5%–18.8%。没有清空系统缓存、锁定频率或测量图形应用首帧。收益不能外推成所有应用、所有机器的固定比例，也没有宣称已达到启动性能极限。按轮成对重采样的探索性 95% 区间见 `analysis.json`；BusyBox 的微小收益对主机噪声较敏感，Bash 子进程场景更稳定。

## 实际改动

- `crates/bridge/src/native.rs`：内部 PE 导出解析借用已固定映射中的名称，发现阶段只为 guest ABI 保留所有权；临时 BTreeMap 也借用名称。公开 `exports()` 仍返回独立拥有的字符串。保留所有导出（包括 Rust 内部符号）的边界、UTF-8、排序、ordinal、地址和节权限检查。
- `crates/bridge/src/module_image.rs`、`crates/loader/src/providers.rs`：绑定时移动导出声明及版本元数据，直接构造 provider 符号列表，省去中间索引和重复克隆。保留地址、对齐、版本、模块寿命、COPY relocation、初始化和 fork 注册语义。
- `libs/ld-linux-x86-64/src/provider.rs`：原生 provider 复用 `Library::open()` 已获得且由模块所有者持有的规范路径，注册阶段不再重复打开文件来 canonicalize。其他 provider 仍走原有路径解析；同一文件被不同 SONAME 注册仍被拒绝。共享库目录也只解析一次。
- `engine/crates/kinakaze-runtime/src/process_creation.rs`、运行时 fork 和 `libs/libc/src/exec.rs`：仅当父进程明确没有控制台，且标准句柄不是控制台句柄时，使用 `DETACHED_PROCESS`。有控制台或状态不确定时保留 `CREATE_NO_WINDOW`；每次创建重新检查，避免缓存附着状态。保留标准句柄传递、挂起启动和 exec/fork 握手流程。

控制台判断还考虑父进程本身的附着状态：即使 stdin/stdout/stderr 都重定向，也可能有其他 guest fd 指向控制台。相关平台语义参见 [Microsoft 进程创建标志](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags) 和 [GetConsoleCP](https://learn.microsoft.com/en-us/windows/console/getconsolecp)。

## 收益证据与剩余瓶颈

10 次外部命令场景的 Windows Job 总进程数在所有样本中从 **43 降至 23**，省掉了 20 个额外控制台进程。累计 CPU 时间中位数由 1429.69 ms 降到 1156.25 ms（下降 19.1%）；page fault 计数中位数由 257079 降到 201423.5。计数包括软缺页，不能当作物理磁盘缺页。

另行开启分阶段日志，各版本 5 次测量，BusyBox 路径的阶段中位数如下。分阶段日志会扰动整体耗时；阶段存在包含关系，不能全部相加。

| 阶段 | 基线 | 优化版 |
| --- | ---: | ---: |
| providers-discover | 16.55 ms | 15.17 ms |
| providers-bind | 16.31 ms | 12.22 ms |
| providers-total | 34.60 ms | 28.55 ms |
| manager-start | 9.98 ms | 10.06 ms |
| worker-open-runtime | 4.82 ms | 4.72 ms |

Provider 总阶段下降约 **17.5%**，仍是新 worker 启动的最大可见开销。进一步压缩需要处理原生 DLL 装载和每个新进程的 provider 重建，涉及 dlopen、对象导出和 fork 恢复的完整语义；本轮保留完整发现、校验和所有权流程。

第二轮路径复用与临时分配优化还做了独立对照：第一轮优化版与最终版各 20 次 BusyBox 启动，中位数 **64.95 → 62.65 ms**，下降 3.5%。该组与主表属于不同时间批次，不混用绝对耗时计算收益。

## 功能验证

- 桥接测试：**12 通过**。新增正常/畸形 PE、借用与拥有名称寿命、批量绑定地址和模块寿命覆盖。
- 链接器测试：**36 通过**。规范路径索引在普通解析、缓存路径两种模式下均保留别名冲突检查，原有版本、COPY 和模块引用测试通过。
- 内核测试：**74 通过、1 个既有忽略**。新增真实子进程检查：无控制台与隐藏控制台父进程的策略选择、重定向 stdin/stdout/stderr 往返正常。
- 7 类真实 guest 回归通过：StandardAbiProbe、loader-entry、environment fork、DailyToolsProbe、PythonRuntimeProbe、ForkIoProbe、StartupProcessProbe。
- 通用 init 进程池：**16 项通过**，包括独占预留与释放、连接 EOF、环境隔离、应用并行、池补充、空闲 worker 崩溃恢复、关闭时清理。混合应用使用真实 Linux Java 25，检查 JIT、线程、文件及 8 次 ProcessBuilder 子进程。
- `StartupProcessProbe.py` 在基线和最终版均通过：posix_spawn 文件操作与非零退出码、fork/exec 后控制终端、输入、回显禁用和恢复、标准输出和标准错误。
- 最终工作区 `cargo check` 通过；涉及文件 rustfmt 和 diff 空白检查通过。

PTY 测试校准：初版直接比较整个 termios 列表，在两版都失败；`tcsetattr` 会把独立返回的输入/输出速率编码进 `c_cflag`。最终测试直接核对实际速率、所有其他字段，并屏蔽 `c_cflag` 中重复的波特率位；初版和诊断日志也保留在 artifacts，未修改终端实现。

这些验证未发现本轮引入的功能回归，覆盖范围不等于所有软件兼容性保证；没有执行完整桌面、GPU 或音频应用回归。

## 产物与复现

所有实验位于 `F:/crysoacu2/artifacts/startup-extreme-20260926/`：

| 路径 | 内容 |
| --- | --- |
| `baseline-dist/` | 优化前固定 Release 基线 |
| `candidate2-dist/` | 最终优化 Release，worker/init/native DLL |
| `source/` | 构建和测试使用的固定源码 |
| `paired2/*/results.json` | 正式交替测量、逐次结果、二进制 SHA-256 |
| `profile2/` | 独立分阶段计时原始日志 |
| `incremental/results.json` | 第二轮优化相对第一轮的独立对照 |
| `regressions-candidate2/` | guest 和进程池回归日志 |
| `terminal-results.json` | 基线和最终版终端回归结果 |
| `kernel-tests.log`、`link-tests.log` | 原生单元测试日志 |
| `analysis.json`、`analyze.py` | 汇总统计和计算脚本 |
| `candidate.patch`、`candidate2-incremental.patch` | 相对固定背景的两轮变更 |
| `original-source-hashes.json`、`candidate2-files.json` | 源码指纹 |

工作区原有其他改动很多，因此基线与优化版均使用固定背景构建。最终检查时，本轮涉及文件与测试源码相同；运行时 `lib.rs` 只有 import 列表换行差异。保留了工作区其他改动，未覆盖默认 `dist`。

从项目根目录重新测量：

```powershell
python artifacts/startup-extreme-20260926/run_paired.py --candidate candidate2-dist --tag rerun
python artifacts/startup-extreme-20260926/run_regressions.py candidate2
```

性能脚本使用 `F:/crysoacu2/artifacts/debian-standard-ready/rootfs`。Java 进程池回归使用已有的 `E:/Naka/crysoacu/crysoacuv2/artifacts/guest-root`，并通过本机 javac 编译 JavaRuntimeProbe；搬到其他机器时需要调整这些实验脚本的 root 路径。启动示例：

```powershell
& F:/crysoacu2/artifacts/startup-extreme-20260926/candidate2-dist/worker.exe run `
  --root F:/crysoacu2/artifacts/debian-standard-ready/rootfs `
  --dist F:/crysoacu2/artifacts/startup-extreme-20260926/candidate2-dist `
  -- /bin/bash --noprofile --norc -c 'echo STARTUP_OK'
```
