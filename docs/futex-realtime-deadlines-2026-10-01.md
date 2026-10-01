# futex 绝对实时时限

此前 raw futex 将绝对 CLOCK_REALTIME 时限一次性换算为单调时长；没有事件
唤醒时，等待无法随着墙钟调整到期时间。现在复制 timespec 后保留原始绝对
目标，等待和到期检查共用 Deadline。接入 raw 202 的 WAIT_BITSET、LOCK_PI、
LOCK_PI2、WAIT_REQUEUE_PI，以及 raw 449 waitv、raw 455 wait 和 pthread
PRIO_INHERIT timedlock/clocklock。PRIVATE、无标记匿名及共享键使用同一规则。

相对 WAIT 与单调时限仍保留原始预算。参数复制、错误优先级、信号重启和
已提交唤醒优先于超时的队列规则继续由原入口和事务处理。

VFS 新增绝对计时器等待：Unix 时间转换成 1601 年起算的 100 ns FILETIME，
向未来取整，计时器与唤醒、信号、所有者死亡句柄进入同一个 wait-any 集合。
高精度计时器不受支持时使用普通绝对计时器；创建或设定失败返回 WAIT_FAILED，
不降级成丢失墙钟调整语义的相对睡眠。关闭临时句柄时保留 Win32 错误码。

Windows 对绝对计时器的时钟调整规则依据
[SetWaitableTimer 文档](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer)。

## 验证范围

原生测试覆盖绝对计时器自然到期、就绪源优先、100 ns 边界、无效句柄，
以及 raw 四类等待的两种时钟/键配置、PI 忙锁超时保留所有者、重排队后
超时清除源绑定和目标记录。墙钟向前/向后调整的剩余量有确定性测试。
相关 futex 与 pthread 回归使用同一成功 Cargo JSON 冻结图，两种优化配置
分别执行：libc 各 103 项相关测试、pthread 各 59 项通过，计时器专项 2 项通过，
零失败；分别忽略 18、4 项助手/性能项。实际结果和镜像 SHA-256 见
[验证索引](measurements/futex-realtime-deadlines-2026-10-01.json)。

本阶段未修改宿主系统时间，尚未做真实墙钟跳变的跨进程端到端试验；也未
重建完整 guest 发行图或测量此修改的吞吐。已有 guest/性能记录保留其原图
边界。全部 syscall 的 3,750 项路径审查、其余 futex 兼容与逐路径性能验证
仍待完成。
