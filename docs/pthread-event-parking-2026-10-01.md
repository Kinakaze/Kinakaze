# pthread 事件等待后端

默认 `KINAKAZE_PTHREAD_PARK_OPT=1`，设置为 `0` 保留 timed mutex 的
TryAcquire/backoff 和 condition 的 100 ms 原生等待对照。

`libs/libpthread/src/parking.rs` 统一私有 timed mutex 和 condition 等待。
每个宿主线程缓存一个 auto-reset 事件及按需创建的 deadline timer。
timed mutex 在最后一次 TryAcquire 之前发布记录；mutex 解锁后通知等待者。
condition 在解锁之前登记，因此其隐式解锁也会唤醒 timed mutex。
signal/broadcast 选择当前队列成员；取消直接设置线程事件，重新取得 mutex
后执行清理，已选中的取消者补发一次 signal。返回及执行 guest handler 前
退休记录，避免 ExitThread 跳过 Rust 析构而留下悬挂队列成员。

普通无竞争 SRW acquire 保留原路径。新队列仅服务已有的进程私有 pthread
对象；pshared、PI、robust pthread 属性和状态仍属于后续工作。fork 子进程
清空等待队列，并避免关闭父进程的事件句柄。guest fork、GNU cleanup 和
信号探针还需针对完整发行包重新验证；本记录不将原生测试等同于客体证据。

过期 condition deadline 在两种模式下都会解锁再重锁。取消时 mutex 必须在
清理回调开始前重新取得，依据 [POSIX 条件等待规范](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_cond_clockwait.html)。
realtime 使用绝对 Windows timer，monotonic 使用相对 timer；定时器不执行
周期唤醒。绝对 timer 随宿主系统时间调整，依据
[SetWaitableTimer 文档](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer)。

隔离目录从 `198838d` 构建，仅加入此 pthread 修改。同一组冻结的 release
产物在默认及关闭开关时均为 **45 passed、0 failed、1 ignored**。原有 41 项
通过公开入口执行；新增 4 项直接测试事件后端，覆盖多个 timed waiter 的
竞争、condition 隐式释放与 ERRORCHECK 重锁、超时退休及已选中取消者的
替代唤醒。真实 pthread 的 TLS destructor 检查取消清理线程持有 mutex。

配对基准每次创建新测试进程，交替开关，1 轮预热、5 轮采样。同一二进制
SHA-256：`cb0c6945e44db338779c88f9d1ddb3c6dee5d21f0dc1b3f7b7801b4dedd52048`。
每轮有 20 个等待线程；各次 mutex 被另一线程持有约 25 ms。

| 指标（20 次总量的中位数） | 对照 | 事件后端 |
| --- | ---: | ---: |
| 解锁至取得 mutex 的延迟 | 17.8114 ms | 0.4515 ms |
| 等待线程执行周期 | 20,659,445 | 4,708,778 |

延迟约为对照的 1/39.45，周期数下降约 77.2%。GetThreadTimes 在此短样本中
两边都报告 0，不能据此声称没有 CPU 消耗；使用 QueryThreadCycleTime 保存
周期数，并保持周期和纳秒为不同单位。这些结果不证明客体吞吐、所有锁的
公平性或端到端性能。完整回合和依赖哈希见
[测量证据](measurements/pthread-event-parking-2026-10-01.json)。

本地完整产物位于 `artifacts/pthread-event-goal-20261001/`，包含构建 JSON、
源文件 SHA-256、冻结 SONAME 包、两配置测试日志及 `benchmark.json`。

```powershell
python tools/benchmark-futex-queues.py --case pthread --binary artifacts/pthread-event-goal-20261001/binaries/kinakaze_libpthread-6a4d296ac0654593.exe --output artifacts/pthread-event-goal-20261001/benchmark.json --rounds 5
```
