# 进程私有 robust pthread mutex

`pthread_mutexattr_setrobust(..., PTHREAD_MUTEX_ROBUST)` 现在创建可恢复的私有
mutex，支持 NORMAL、RECURSIVE、ERRORCHECK 和 ADAPTIVE 类型。robust 属性
使用 Linux 的单个 32-bit attribute word，设置类型不会覆盖 robust 位。
STALLED/ROBUST 可切换，并提供 `*_np` 兼容别名。pshared 和 PI 尚待实现。

原生 mutex 的 abandonment 检测真实宿主线程退出，覆盖 normal return、
pthread_exit、deferred cancel 和 raw SYS_exit，无需定期探测死亡线程。每个
mutex 保存独立的 POSIX 修复状态：

| 触发 | 返回与所有权 |
| --- | --- |
| 原 owner 未解锁就退出 | 下一个 owner 获得锁，返回 EOWNERDEAD (130) |
| 新 owner 调用 consistent | 恢复健康状态，仍持有锁 |
| 未 consistent 就最终解锁 | 后续 acquire 返回 ENOTRECOVERABLE (131)，不持有锁 |
| 恢复 owner 再次死亡 | 下一个 owner 再次收到 EOWNERDEAD |
| 非 owner unlock/consistent | 分别返回 EPERM/EINVAL |

规则依据 [POSIX mutex lock](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutex_lock.html)。
Windows abandonment 也同时取得 ownership；递归次数由本库维护，避免把
Windows 的无条件递归行为赋给 NORMAL/ERRORCHECK。原生对象行为见
[Microsoft mutex 文档](https://learn.microsoft.com/en-us/windows/win32/sync/mutex-objects)。

普通 SRW mutex 保留现有实现，robust mutex 使用独立原生对象。owner liveness
复用 pthread 调度注册表已经保留的真实线程 handle；每次 acquire 不再复制
线程 handle。超时等待复用每线程 timer，保留立即取得锁时不检查 timespec
的规则，以及 REALTIME/MONOTONIC 的绝对 deadline 预算。

robust condition 等待在两种性能开关模式下均使用事件后端，因为原生 mutex
不能作为 SRWLOCK 交给 SleepConditionVariableSRW。取消重新取得 mutex 后
才执行 GNU cleanup。重新取得 abandoned mutex 返回 EOWNERDEAD，并保留
ownership 给 caller 修复；此时不会为信号 handler 释放锁而意外进入不可恢复
状态。两种后端的 signal/broadcast 均可通知 robust 等待者。

客体探针揭示 raw SYS_exit 先前没有发布 join 结果，导致 pthread_join 返回
EINVAL。现在 raw 退出只退休内部等待/线程/栈注册；不执行用户 cleanup/TSD
析构，并由 libc 清理 TID、递减任务计数一次。通过 pthread_create 创建的
线程发布初始 NULL join 结果；其他 raw task 不创建无人回收的 pthread result。

fork 快照只传输 address、type、owner、递归深度和 repair state；不传递
原生 handle 或 Rust pointer。子进程建立自己的原生对象，恢复当前线程持有的
锁和 repair state。已死亡的 owner 转换为待修复状态；仍由其他父线程持有的
私有锁保留阻塞状态，与已有 typed SRW 恢复策略相同。退出线程的 custom
stack 范围中的对象元数据在 stack 退休时移除，避免下次 fork 处理旧地址。

这些状态检查与能力实现始终启用；`KINAKAZE_PTHREAD_PARK_OPT=0` 不会取消
robust 语义。本轮不将功能测试的执行时间当成吞吐或公平性证明。完整 PI、
pshared robust mutex，以及 futex PI/requeue PI 仍属于持续优化目标。

## 验证

从 `42aab002` 隔离源码，只加入本记录对应修改，完成 workspace release
构建并整体替换发行目录中的 Cargo 产物。最终 pthread 原生套件两种配置
各为 **51 passed、0 failed、1 ignored**，TLS 为 **25 passed、0 failed**。

真实 Debian guest 两种配置均通过四个探针组：已有 GNU cleanup、robust
condition cancel、事件等待，以及新 robust 探针。新探针每种配置执行 32 组
恢复/不可恢复组合（四种 mutex 类型 × return/pthread_exit/raw SYS_exit/
cancel × consistent/未修复解锁），检查 C++ thread_local/TSD 回调在正常
退出时执行、raw 退出时跳过，并验证 raw join 的 NULL 结果。另覆盖 unlocked、
当前线程 recursive ownership、inconsistent ownership、不可恢复四种 fork 状态；
子进程再次创建 raw 退出的持锁线程并恢复。

首次客体运行保留了 raw join 失败；补齐内部退休后，又保留了宿主 TLS 意外
执行 guest 回调的失败。两项修复后的最终运行全部通过。原生 TLS 回归还
检查线程隔离和当前 C++ destructor runner 内的弃置行为。完整日志及产物
SHA-256 见 [证据](measurements/pthread-private-robust-2026-10-01.json)。

```powershell
python tools/test-pthread-event-guest.py --root artifacts/goal-systemd-idle/debian-root --dist artifacts/pthread-robust-goal-20261001/candidate-verified --output artifacts/pthread-robust-goal-20261001/reproduce --robust-cancel
```
