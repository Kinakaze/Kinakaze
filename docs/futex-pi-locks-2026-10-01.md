# futex PI 锁后端阶段

本阶段接入 raw 202 的 LOCK_PI（6）、UNLOCK_PI（7）、TRYLOCK_PI（8）及
LOCK_PI2（13）。支持 PRIVATE 与无标记键、共享匿名/文件别名、所有者 TID、
WAITERS/OWNER_DIED、无竞争 CAS、阻塞转移、绝对超时和信号后的入口重启。
这是完整 futex 目标中的锁原语阶段，PI requeue、pthread PI 属性及 Linux 实时
调度策略仍需实现；全部 3,750 项 syscall 路径审查仍未据此自动标为完成。

## 状态与崩溃恢复

PI 等待者、所有者和转移日志写入原有双 bank 队列，以同一个命名域 mutex
串行化键检查、用户锁字和记录发布。记录继续保持 72 字节；增加内部角色后，
队列 magic、section、guard 与 park/event 名称升级为 v2，隔离旧版后台。
普通 wake/requeue 拒绝 PI 等待者，忽略 PI 元数据；普通 wait/waitv 的选择与
取消同样忽略元数据。所有等待者统一使用非零 token，零值保留给所有者状态。

unlock 先发布转移意图，再 CAS 锁字、发事件并发布新所有者。提前发事件不
代表成功；等待者取得域 mutex 后读取已提交状态。unlocker 在日志发布前、
CAS 前、CAS 后及最终发布后被 ExitProcess 杀死时，另一进程均能通过自己的
映射别名恢复。等待者监听 park、signal 和原生所有者退出句柄；每次所有权
转移通知仍在排队的线程更新退出句柄，避免继续监听已经释放锁的旧所有者。

锁字通过安全复制读取，写操作使用单条 LOCK CMPXCHG 汇编。VEH 仅匹配该
指令地址，将页失效转为 EFAULT；其他指令的异常继续交给原来的处理器。
写权限预检查同时避免主动消耗 PAGE_GUARD。没有建立长期指向用户页的 Rust
原子引用。无竞争路径避免任务表加载和原生调度器查询。

## 任务身份与调度

任务表只保存 PID namespace、guest TID、host PID/TID、线程创建时间及调度
数值。gettid 首次公布当前任务；重复调用使用线程缓存。raw clone 已经在
客体入口前调用 gettid。fork 的子钩子重置 native leader 身份和本地缓存，
新 worker 不会把父进程的 native leader 当成自己的 leader。外部进程 leader
通过共享身份表解析，不会直接把 guest PID 当作 Windows TID。

捐赠计算沿等待者到所有者传播至固定点，并实际调用 SetThreadPriority。
任务表分别保留调度 base 和继承值；超时、退出、取消及 unlock 重新计算并
恢复 base。pthread_setschedparam 的 base 更新通过内部回调进入同一个后端，
活动捐赠不会被普通 OTHER/0 请求清掉。两把锁构成等待环时返回 EDEADLK。

目前允许的 Linux 调度策略仍是 OTHER/0，其 futex 队列保持 FIFO。Windows
原生优先级捐赠和恢复有实际测试，但不等价于 Linux FIFO/RR 调度保证；现有
FIFO/RR 请求仍返回 EPERM。实现依据为
[Linux 6.12 PI](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/pi.c)、
[sys_futex 入口](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)
及 [Windows 原生调度](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority)。

## 验证与测量

最终固定原生图在 KINAKAZE_FUTEX_OPT=0/1 下分别通过 libc 619 项、pthread
51 项，零失败；忽略项分别为 17 和 1，包括已有性能/helper 测试。新增
14 项功能测试覆盖所有者位与错误顺序、busy/trylock、到期取消、普通 API
混用、三个 FIFO 等待者、死亡转移、真实原生捐赠和恢复、活动捐赠期间修改
base、嵌套传播、两锁环、跨进程四个崩溃时点以及普通/向量 token 隔离。

原生测试与客体图分别从成功的完整 Cargo JSON 冻结，保存全部镜像和日志的
SHA-256。早期测试编译的 E0382 日志保留，不作为通过证据。详细结果和原始
测量索引见 [测量记录](measurements/futex-pi-locks-2026-10-01.json)。

完整 workspace release 构建后冻结 38 个镜像。真实 Debian 的 pi、timeout、
signal、scalar、vector、requeue 六类探针在两配置共 12 个会话均通过，发行
镜像哈希保持不变。PI 探针包含 private/unflagged 三种获取命令、三个 FIFO
等待者、16 组 LOCK_PI/LOCK_PI2 信号与 SA_RESTART/超时参数重拷贝组合、
fork 后 leader TID、不同 VA 的文件别名及 raw exit_group 后的死亡接管。
会话耗时含初始化与其他客体工作，不能用作 PI 性能增益。

固定原生 release 图在编译与客体探针均结束后顺序运行，每配置先 warmup，
再进行五轮交替测量。以下是每轮 2,000 个 raw 获取/释放对的中位毫秒：

| 键 | 获取命令 | 关闭开关 | 默认开关 | 关闭/默认 |
|---|---:|---:|---:|---:|
| 无标记 | 6 | 6.006 | 5.265 | 1.14 |
| 无标记 | 8 | 5.991 | 5.223 | 1.15 |
| 无标记 | 13 | 5.977 | 5.343 | 1.12 |
| PRIVATE | 6 | 4.347 | 3.729 | 1.17 |
| PRIVATE | 8 | 4.381 | 3.664 | 1.20 |
| PRIVATE | 13 | 4.401 | 3.805 | 1.16 |

此表比较同一 PI 后端的开关配置，主要包含进程/线程身份缓存差异。使用原生
默认测试域，未加入无关等待行；不作为旧 ENOSYS 后端与新 PI 后端的速度
比较，也不外推竞争、跨进程和全部 syscall 路径。完整样本与日志 SHA 在
上述测量 JSON 中保留。

## 继续工作

WAIT_REQUEUE_PI/CMP_REQUEUE_PI、pthread PI 属性、Linux 实时
策略、跨 PID namespace 层级的所有者视图，以及更多分配失败、非合作式退出
和内存权限竞争路径仍需完成。死亡 PI 等待行由 PI 操作/退出清理维护，普通
wake/requeue 不代替这项清理。原有 futex/VFS/io 的性能与所有 syscall 的
JIT、SIMD、模板生成及逐路径审查继续推进；本阶段没有宣称这些目标全部完成。

## 与最新 main 集成

以上原生/客体固定图验证对应 63bc6f9 的源树（基础 1153b90）。随后合并
22df00b 的最新 main，得到 721624a，并通过 release 生产编译检查（14.07 秒）。
本次合并同时包含独立完成的普通/robust 共享 pthread mutex 后端，见
[共享 mutex 阶段记录](pthread-shared-mutex-2026-10-01.md)；pthread PI 属性
仍未接入。此处的生产检查不替代对合并后新源树重新执行整套运行测试。
