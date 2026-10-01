# futex PI requeue

本记录对应原 v3 队列协议的固定图；后续代理恢复、批量路径及当前验证范围见
[PI 重排队边界与集成检查](futex-pi-requeue-2026-10-01.md)。

raw syscall 202 已接入 WAIT_REQUEUE_PI（11）和 CMP_REQUEUE_PI（12）。源条件等待进入共享数值队列，重排时代理获取 PI 目标，或进入目标锁的既有等待队列；成功返回的等待线程已拥有目标锁。PRIVATE 与无标记操作保留各自的键空间，匿名共享和同文件不同映射地址继续使用共享键。

每个源等待者暂用两个 72 字节记录：源队列行和目标身份绑定，均不含 guest/Rust 指针或 native handle。转移保留 token 并追加在目标已有等待者之后，保持 OTHER/0 队列的 FIFO。普通 wake/requeue 与 PI requeue 不混用；错误目标不消费源等待者。wake 参数必须为 1，额外重排数必须非负；额外数为零时仍处理第一个等待者。队列、guard、park 名称升级为 v3，任务表升级为 v2，隔离使用不同记录角色和锁协议的旧进程。

空目标上的代理获取将迁移、所有者状态和 CAS 恢复日志一起发布。先通知源 event 再提交，等待者读取已提交队列；重排进程在日志前、CAS 前、CAS 后和最终发布后被 ExitProcess 杀死时，都不会误报拥有权或丢失恢复通知。重排进程死亡不等于锁所有者死亡，正常代理获取不设置 OWNER_DIED。竞争目标复用 PI 捐赠、所有者退出监听、超时取消和 base 恢复。

绝对超时支持 monotonic/realtime。迁移前的信号先移除源 token，再重新进入 syscall 并重拷贝 deadline，即使没有 SA_RESTART；迁移后的信号取消目标等待并返回 EAGAIN。拥有权已转移时，成功优先于同时发生的超时或信号。规则参照 [Linux WAIT_REQUEUE_PI 实现](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/requeue.c)及 [WAIT_REQUEUE_PI 手册](https://man7.org/linux/man-pages/man2/FUTEX_WAIT_REQUEUE_PI.2const.html)、[CMP_REQUEUE_PI 手册](https://man7.org/linux/man-pages/man2/FUTEX_CMP_REQUEUE_PI.2const.html)。

固定 release 原生图在 KINAKAZE_FUTEX_OPT=0/1 下分别通过 libc 625 项、pthread 59 项，零失败；分别忽略 19、4 项性能/helper 测试。新增测试覆盖验证顺序、两种时钟、实际拥有权、目标已有等待者优先于更早的源 token、超时后的真实 native 捐赠恢复、错误目标重试和四个真实 subprocess 崩溃时点。完整原始日志与二进制 SHA-256 保存在本轮 artifacts；测量索引和 guest 验证结果见 [记录](measurements/futex-requeue-pi-2026-10-01.json)。

资源限额继续沿用共享 futex bank 的 2048 条记录和 PI 任务表的 4096 项，源等待者的绑定计入前者。满容量时清理已死亡的源行及绑定，不静默丢弃活跃等待者。pthread PI 属性已由[后续接入阶段](pthread-pi-mutex-2026-10-01.md)实现；Linux 实时调度策略和整体 syscall/VFS/I/O 优化目标仍需继续完成。

完整 workspace release 构建成功后，从 Cargo JSON 冻结 38 个发行镜像。真实 Debian guest 的新 requeue-pi、既有 pi/timeout/signal/scalar/vector/requeue，以及共享 pthread condition/robust cancellation，在两种开关配置共 18 行全部通过；各报告的发行文件 SHA-256 完全一致。新探针同时覆盖迁移前后信号、两种 SA_RESTART 设置、private/unflagged、两种时钟、三线程 FIFO 以及 fork 后同文件不同 VA 的空闲/竞争目标。

固定原生图预热一对，再交替测量 5 对，每对各执行 1000 个源入队、代理获取、拥有权消费与 unlock：关闭 KINAKAZE_FUTEX_OPT 的中位耗时为 16.692 us/组，默认配置为 15.8357 us/组，约 1.05 倍。计时包含组件内的域事务和队列操作，不包含 syscall 入口、线程停车或应用负载；比较的是同一新后端的开关配置，不把旧 ENOSYS 当作性能基线。复现使用 `tools/benchmark-futex-queues.py --binary <frozen-libc-test.exe> --case requeue-pi --output <report>`；guest runner 使用 `--probe requeue-pi`。
