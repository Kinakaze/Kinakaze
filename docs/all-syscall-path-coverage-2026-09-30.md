# 全部 syscall 与 I/O 路径覆盖

10 月 1 日阶段已接入 raw 202 的 LOCK_PI、TRYLOCK_PI、UNLOCK_PI 与
LOCK_PI2，补充共享所有权转移日志、原生捐赠、死亡接管及信号入口重启。
最终固定图在两配置各通过 libc 619 项、pthread 51 项；真实 Debian 的
pi、timeout、scalar、signal、vector、requeue 六类探针在两配置均通过。
实现、边界与测量见 [PI 锁阶段记录](futex-pi-locks-2026-10-01.md)。
WAIT_REQUEUE_PI/CMP_REQUEUE_PI 已在后续接入；两配置各通过 libc 625 项、pthread 59 项，18 行真实 guest 回归通过，见[PI requeue 阶段记录](futex-requeue-pi-2026-10-01.md)。
旧式超时入口与 raw 202/455 的独立证据仍见
[超时入口阶段记录](futex-timeout-entry-2026-10-01.md)。
覆盖工具当前记录 304 项已分发、1 项明确拒绝、70 项缺少分发；375 个 syscall
的 3,750 项完整路径审查仍保留待验证状态。

持续目标覆盖所有 syscall 入口和 VFS、内存 I/O、Unix I/O 的各种执行路径，
包含 JIT/AOT、分发、参数校验、阻塞/非阻塞、共享/私有、并发、生命周期、
错误与回退。完整 futex/PI/robust、正确性验证及逐路径性能测量仍是完成条件。

`tools/audit-syscall-paths.py` 读取仓库内 Linux 6.12 x86-64 LP64 的完整 syscall
表，关联当前 dispatcher 行号、调用入口、设计所有者及 Linux 验证要求。
375 个 syscall 各有 10 项路径审查，共 3,750 项。每项保留证据字段及未完成
状态；分发存在、共享汇编桥模板或组件测试通过均不自动证明该 syscall 的
所有语义、路径或性能已经验证。缺失入口同样保留在清单中。

```powershell
python tools/audit-syscall-paths.py --output artifacts/syscall-all-paths-20260930/coverage.json --csv artifacts/syscall-all-paths-20260930/coverage.csv
```

JSON 保存源文件 SHA-256，CSV 可直接筛选 syscall、所有者和待验证路径。
该快照是覆盖清单，尚不是已完成的优化清单。后续每个路径需要补充明确的
适用技术判断、正确性/竞争/客体证据及对照测量；不适用的技术也需要理由。

## futex 的键规则

Linux 6.12 的 `get_futex_key` 对无标记匿名页增加 `FUT_OFF_MMSHARED`，而
`PRIVATE_FLAG` 路径不增加该标签，`futex_match` 同时比较 offset。因此，不能
为了“互通”合并两类等待队列。新旧 futex API 在相同 flags/键规则下互操作，
同时必须保留 PRIVATE 与无标记操作的隔离。

依据：[Linux 6.12 键解析](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/core.c)
与 [键比较](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/futex.h)。

## 多地址事件等待

raw 449 接入 `futex_waitv`，接受最多 128 个 U32 描述符，逐项复制和校验
flags、reserved、值宽度，支持私有/无标记混合、绝对超时及信号。
所有共享记录使用同一个 token/事件，`reserved` 保存原始向量索引。
唤醒依据已提交队列中缺失的成员，避免把崩溃 waker 的提前 SetEvent 当成成功。
重排队保留索引；返回及执行信号处理前移除全部兄弟成员、释放命名句柄。

`KINAKAZE_FUTEX_OPT=0` 使用逐项入队对照；默认路径在命名域锁与进程队列锁
下批量发布。两种路径保留相同 syscall 入口和 Linux 返回语义，控制路径也
支持完整 128 项，不依赖 128 个原生等待句柄。

依据：[Linux 6.12 waitv ABI](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)
与 [多地址等待、清理和信号处理](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/waitwake.c)。

专项性能探针经过实际 raw 449 分发，测量过期绝对超时下的完整入队/退休，
逐次检查 ETIMEDOUT 并确认没有剩余成员。它不测线程调度延迟或跨进程吞吐。

```powershell
cargo test -p kinakaze-v2-libc --release --locked --no-run --target-dir target/syscall-all-paths
python tools/benchmark-futex-queues.py --case waitv --binary target/syscall-all-paths/release/deps/kinakaze_libc-b55280b0b58bcfa2.exe --output artifacts/syscall-all-paths-20260930/waitv-benchmark.json
```

本记录不宣称 futex PI、robust pthread 或所有 syscall 已完成。

## waitv 初始阶段验证（保留历史）

在独立 `target/syscall-all-paths` 中，最新 release 原生回归通过 1,569 项：
libc 578、VFS 844、guest-engine 60、kernel 87。共 31 项被忽略，是独立组件
基准、子进程助手或原有专项运行测试。关闭 `KINAKAZE_FUTEX_OPT` 后，同一固定
libc 二进制的 578 项再次通过。清单工具的两项检查确认完整表覆盖和重复分发拒绝。

原生结果：`artifacts/syscall-all-paths-20260930/native-tests-final.json`、
`libc-control.json`、`libc-control.log`。等待组专项覆盖跨进程唤醒、域隔离、
waker 在 SetEvent 后/提交前死亡、部分银行损坏、取消后重复查询以及 requeue。
这些原生结果不能替代真实客体映射别名/fork 探针。

raw 449 在同一固定二进制中交替开关，一轮预热、五轮记录的组件中位数如下。
每项执行完整入队/退休，使用过期的绝对超时，不进行线程调度延迟测量。

| 工作量 | 对照 | 批量入队 | 时间比 |
| --- | ---: | ---: | ---: |
| 8 项私有，1,000 次 | 20.904 ms | 13.342 ms | 1.57 |
| 128 项无标记，256 次 | 2,665.077 ms | 65.544 ms | 40.66 |
| 32 项混合，1,000 次 | 466.221 ms | 63.162 ms | 7.38 |

`waitv-benchmark.json` 保存二进制/依赖 SHA-256、各轮数据及原始中位数。
这些数字不证明跨进程吞吐、端到端安装或所有 syscall 均达到同等加速。

`tests/guest/FutexVectorProbe.py` 与 `tools/test-futex-vector-guest.py` 检查实际
Debian 客体中的 128 项混合等待、MAP_SHARED 文件映射别名以及 fork 后父子
进程唤醒，分别启动开/关两种配置。由于 native 开关可能在预热时缓存，每种
配置在启动 init/worker 前设置宿主环境，并使用独立的进程池。

两种配置的实际 Debian 探针均通过，结果为
`artifacts/syscall-all-paths-20260930/guest-vector-full/report.json`，各自的
控制器 stdout/stderr 保存在 `control/` 与 `candidate/`。该报告保存全部
发行目录镜像 SHA-256。两个会话的单次执行时间不作为性能对照结论。

候选目录 `candidate-full` 由本轮成功的全工作区 release 构建生成；
`guest-build-full.jsonl` 与 `guest-staging-full.json` 保留编译产物及 staging
哈希。早期局部候选包因新旧 Rust DLL 导入不一致无法预热，未用于上述结果。
