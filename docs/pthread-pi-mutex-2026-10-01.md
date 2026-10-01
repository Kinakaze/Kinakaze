# pthread PI mutex 接入

后续无竞争路径优化、v6/v5 共享段代际与新验证见
[热路径记录](pthread-pi-hotpath-2026-10-01.md)。本文保留本次接入的历史构建证据。

`pthread_mutexattr_setprotocol(PTHREAD_PRIO_INHERIT)` 现在保存属性，并通过
libc 安装的回调进入已有 futex PI 后端。普通、递归、ERRORCHECK、ADAPTIVE
四种 mutex 均支持 private/shared 与 stalled/robust 组合；PRIO_PROTECT
仍返回 ENOTSUP。libpthread 不反向链接 libc，安装回调只执行原子指针存储。

## 布局和所有权

对象保留 Linux x86_64 的 40 字节布局。属性协议位为 28–29，mutex 的 PI
标志为 32、robust 为 16、共享为 128。与 glibc 一致，robust 使用无 PRIVATE
标记的 futex 键，即使其属性请求 private。word 保存 TID、WAITERS 和
OWNER_DIED，后续字段记录递归计数、所有者和使用者数；共享对象不保存宿主
句柄、Rust 锁或其他进程的指针。

布局依据为 [glibc 初始化代码](https://github.com/bminor/glibc/blob/master/nptl/pthread_mutex_init.c)
及 [glibc 2.36 获取代码](https://github.com/bminor/glibc/blob/glibc-2.36/nptl/pthread_mutex_lock.c)。
实现沿用本项目的数值 bank、故障保护 CAS 和命名事件。

无竞争获取也先预留记录、保留用户字、公布获取日志，然后赋予 TID。持有期间
保留一条数值所有者记录，使另一个存活参与者在所有者被 TerminateProcess
结束后仍能识别其宿主身份和出生时间。初始化者先打开 domain bank，避免
首次持锁的子进程退出后该 section 随最后一个句柄消失。正常 unlock 清理
此记录，排队交接继续携带标志。线程退出保留日志引用的等待行，让下一调用者
完成尚未公布完的获取，避免产生没有目标的 journal。

新所有者标志使用 bit 22，与最新 main 的 requeue 日志位隔离。共享队列、
guard、park 升级为 v5，任务表升级为 v4；任务缓存同时验证 host 和 domain。
合并保留最新 requeue 的故障回滚、错误顺序和源码测试。

## 锁契约

递归溢出返回 EAGAIN，ERRORCHECK 自锁返回 EDEADLK，NORMAL 自锁继续等待，
trylock 自锁返回 EBUSY。错误所有者不能 unlock。竞争沿现有任务图实际捐赠
Windows 线程优先级，unlock、超时和退出恢复基础值。

robust 所有者死亡后返回 EOWNERDEAD，由 consistent 修复。未修复的 unlock
使对象持续返回 ENOTRECOVERABLE；已经排队并得到 futex TID 的线程也先交还
所有权，再返回此错误。stalled 所有者死亡后保持等待，timedlock 可到期。
合作式退出通过线程本地持锁记录处理，非合作式退出通过保留的数值身份恢复。

clocklock 支持 realtime 与 monotonic。立即取得锁时不检查 timespec 内容，
需要等待时复制并验证绝对时限；pthread mutex 获取不因普通信号返回 EINTR。
条件等待使用事件路径释放和重新获取 PI mutex，两种 parking 开关均能运行
GNU 取消清理。协议 getter/setter 的 GLIBC_2.2.5 / GLIBC_2.34 别名和生成器
规则一并保留。

## 验证

固定合并源树为 `3f23211`，功能提交为 `24051a6`。从成功 Cargo JSON 冻结
全部原生依赖和 38 个 guest 构建镜像，记录文件、构建日志和探针的 SHA-256。

| 配置 | libc | pthread | 失败 |
|---|---:|---:|---:|
| FUTEX / PARK / SHARED_CACHE = 0 | 643 | 59 | 0 |
| FUTEX / PARK / SHARED_CACHE = 1 | 643 | 59 | 0 |

libc 另有 20 项、pthread 另有 4 项已声明忽略的 helper/性能测试。新增 7 项
功能测试覆盖 16 种属性组合、递归/错误所有者、双时钟、自锁到期、实际原生
优先级捐赠和恢复、robust 修复/poison、stalled、条件重取、已经排队的 poison
接收者，以及四个获取日志时点和跳过 DLL/TLS 清理的 TerminateProcess。

真实 Debian guest 共 22 行通过：共享 PI mutex、共享 PI 条件与别名、private
及 shared 的 GNU robust PI 取消清理各在两个配置运行；raw PI、PI requeue、
scalar、vector、普通 requeue、timeout、signal 七类回归也各运行两个配置。
发行图哈希全程一致。导出生成器 25 项测试通过。早期失败日志保留，不计入
上述通过项；详细证据见 [测量记录](measurements/pthread-pi-mutex-2026-10-01.json)。

## 后续边界

Linux FIFO/RR、PRIO_PROTECT、外部 glibc robust 链表混用和跨 PID namespace
查询继续属于总目标。上述恢复依赖存活参与者保留 domain bank；全部旧参与者
退出后重新映射持久文件 mutex 的恢复尚未建立。无事件的长时间 realtime
等待期间发生大幅时钟跳变也尚未验证。bank 仍限 2048 条数值记录、4096 个
任务身份，持有 PI mutex 会占用所有者行。

本次接入没有新增速度结论。guest 会话耗时包含初始化，不能充当锁的性能
测量；JIT/SIMD、VFS/I/O 极限优化及全部 syscall 路径审查继续推进。
