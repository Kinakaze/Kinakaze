# PI 重排队 v4 的完整图验证和批量分发测量

以 main 4e5914f 的 v4 队列、v3 任务表为固定源树，补齐真实 Debian、
跨进程只读别名恢复、死亡源绑定的容量回收以及实际 raw syscall 批量测量。
本记录只证明下述范围；375 个 syscall、3,750 项完整路径审查继续保留。

## 固定图与正确性

从成功 Cargo JSON 冻结所有原生依赖和完整 workspace 的 38 个发行镜像。
最终原生图在 FUTEX_OPT=0/1 下各通过 libc 638 项、pthread 59 项，零失败；
分别忽略 21、4 项已声明的 helper/性能项。原生镜像在回归及基准后哈希未变。

只读助手通过与父进程不同 VA 的同一 Windows section 操作共享数值键。
活助手代理 CAS 返回 EFAULT 时保留源 token，父进程经可写别名重试并获取；
只读助手在 CAS 前被结束时，父进程经可写别名完成已提交日志，且不会把
重排队者死亡当成锁所有者死亡。

容量助手在独立 helper 域内检查 32,768 条记录的限额：请求超额返回 ENOMEM
并保留活源与绑定；第二个助手先登记并报告 ready，父进程持域锁确认两条
活记录后，用 TerminateProcess 结束助手，再申请额度并确认源与绑定一起
回收。早期未钉住存活登记的失败助手日志保留，不算通过证据。

完整 workspace release 构建通过（2 分 47 秒）。同一个冻结发行图通过 18 行
真实 Debian 会话：requeue-pi/pi/timeout/signal/scalar/vector/requeue 两配置，
以及共享 pthread condition 和 shared robust GNU 取消各两配置。所有报告的
发行图 SHA-256 完全相同。新增 guest 边界包括 PRIVATE 未映射目标、同文件
别名的错误顺序、共享只读目标、PRIVATE mprotect 写故障后的重试，以及
有父进程等待线程时 fork 的只读别名拒绝与父进程重试。

## raw CMP_REQUEUE_PI 批量时间

新增 `requeue-pi-batch` 基准通过 native raw 202 分发。每个配置测试无标记
匿名和 PRIVATE 的 8、32、128 等待者，每项 8 次。目标锁在迁移计时期间
由父线程持有；源入队、初始化、锁获取、后续所有权交接和线程回收均在计时
之外。每次检查迁移额度、源清空、目标行数、最终成功获取/释放及完整元数据
退休。它不测 guest JIT 入口、应用吞吐、空闲代理获取或跨进程迁移。

同一冻结二进制先预热一对，再进行五对交替测量。下表为每轮 8 次调用的
平均时间，再取五轮中位数，单位为每次 CMP_REQUEUE_PI 的微秒：

| 键 | 等待者 | 逐项对照 | 默认批量 | 对照/默认 |
|---|---:|---:|---:|---:|
| 无标记匿名 | 8 | 300.837 | 134.850 | 2.23 |
| 无标记匿名 | 32 | 2856.262 | 319.087 | 8.95 |
| 无标记匿名 | 128 | 46353.050 | 1308.362 | 35.43 |
| PRIVATE | 8 | 270.350 | 83.625 | 3.23 |
| PRIVATE | 32 | 2907.738 | 315.688 | 9.21 |
| PRIVATE | 128 | 41428.775 | 1476.412 | 28.06 |

计时开关是 KINAKAZE_FUTEX_OPT，两种路径具有相同接口和结果；数值对照
比较同一实现的逐项提交/捐赠计算与默认批量提交。开始的进程快照只发现该
基准助手，结束时没有 cargo/rustc/init/worker/原生测试进程；采样不是连续
全系统负载保证。全部原始样本、源文件、构建图、日志及二进制 SHA-256 见
[验证与测量索引](measurements/futex-pi-requeue-v4-validation-2026-10-01.json)。

```powershell
python tools/benchmark-futex-queues.py --binary <frozen-libc-test.exe> --case requeue-pi-batch --rounds 5 --output <report.json>
```

## 后续 main 与边界

上述原生/客体/性能数据对应 v4 固定图。main 后续已接入
[pthread PI mutex 和 v5 协议](pthread-pi-mutex-2026-10-01.md)；v4 数据不自动
证明后续 v5 图的性能或所有路径。Linux realtime、PRIO_PROTECT、跨 PID
namespace、外部 robust 链表互操作、时钟跳变和所有 syscall/VFS/I/O 的
逐路径验证仍需继续。此阶段没有缩减持续目标。
