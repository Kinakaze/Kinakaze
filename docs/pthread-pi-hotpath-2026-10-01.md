# pthread PI 无竞争热路径

在 `4a82d5b` 的 pthread PI 接入上，默认启用 `KINAKAZE_PTHREAD_PI_OPT`；
设为 `0` 可恢复本轮保守路径。四种属性组合的无竞争 lock/unlock 避免重复
优先级图求解、向当前调用者发送自己的唤醒事件，以及重复提交 PINNED 标志。

获取仍预留数值记录、保留用户字、公布 journal 和所有者，再公布 pthread
字段。优化只在整个 domain 没有 PI 等待行或 journal、所有任务已应用基础
优先级、调度重放标志为零时运行；竞争、取消、死亡和未完成恢复进入原路径。
直接消费当前调用者的等待 token，与随后 owned() 的字段提交合并。所有者
行仍保留，获取过程中被结束的调用者仍由其他参与者完成 journal 恢复。

任务表在发布优先级意图之前设置共享原子重放标志，所有实际 Windows 线程
优先级更新成功后才清除。即使意图已显示基础优先级，而宿主更新中途失败或
进程被结束，也不会错误跳过恢复。检查直接借用受 domain guard 保护的任务
bank，避免复制任务数组和打开每个线程的句柄。unlock 在清理死亡等待者
之前检查条件，避免清掉最后一个 donor 后遗留已提升的宿主优先级。

共享队列、guard、wait、park 的名字和 magic 升级为 v6，任务表升级为 v5。
旧参与者不会写新重放标志，因此隔离共享段代际；存储布局保持一致。

## 测量

使用冻结的 release 原生测试程序，通过真实公开 pthread lock/unlock 接口
测量 private/shared × stalled/robust。每行 500 对调用，先局部预热 16 对；
完整预热一轮，再交替执行开关各五轮并取中位数。32 个后台线程均为实际
存活且已注册的空闲 PI 参与者，测量期间没有其他 PI 等待行。

| 组合 | 空闲线程 | 开关 0，ns/对 | 开关 1，ns/对 | 比值 |
|---|---:|---:|---:|---:|
| private stalled | 0 | 14998 | 7520 | 1.99 |
| shared stalled | 0 | 15397 | 8281 | 1.86 |
| private robust | 0 | 15933 | 8747 | 1.82 |
| shared robust | 0 | 16125 | 8670 | 1.86 |
| private stalled | 32 | 98206 | 7460 | 13.16 |
| shared stalled | 32 | 98498 | 8332 | 11.82 |
| private robust | 32 | 99845 | 8740 | 11.42 |
| shared robust | 32 | 97396 | 8659 | 11.25 |

额外冻结旧实现，仅加入相同的 ignored 测量用例；其五轮中位数分别约为
13.7–14.8 μs/对和 90.0–92.2 μs/对。新二进制的保守路径约慢 6–10%，
其中包含新增的测试构建故障注入检查等差异；不把两者差异归因于单一因素。
上表比较同一个新二进制的开关。以上是 cfg(test) 原生组件测量，不能推断
guest 应用、syscall、VFS 或 I/O 的整体吞吐提升。guest 仅用于功能回归。

## 验证与证据

验证源码为 `4a82d5bce2d7a4ca9b0a6985ee424fabd0f90e5b` 加本轮七个文件
的修改；逐文件 SHA-256、构建日志、完整冻结依赖、原始测量与回归结果见
[记录](measurements/pthread-pi-hotpath-2026-10-01.json)。合并其他 main 提交
不会把这批合并前构建重新标为合并后构建。

FUTEX_OPT × PTHREAD_PI_OPT 四组完整原生配置均通过：每组 libc 645 项、
pthread 59 项；另有 libc 22 项、pthread 4 项声明忽略的 helper/测量用例。
目标 PI 测试在两个开关分别通过 9 项。新增恢复测试注入两个中断的共享
意图状态，并检查实际线程优先级；该用例没有声称在调度更新中途杀死进程。
另一新增用例真正使用 TerminateProcess 结束 donor，跳过 TLS/DLL 清理，
确认清理死亡等待者后 owner 恢复基础优先级。获取故障用例增加直接消费
等待 token 后的退出时点。

完整 workspace release 构建成功，38 个构建产物从同一 Cargo JSON 冻结。
22 行真实 Debian guest 回归全部通过且发行图哈希一致：共享 PI mutex、
共享 PI 条件、shared/private robust GNU 取消清理各运行两个开关；raw PI、
PI requeue、scalar、vector、普通 requeue、timeout、signal 各运行两次。

本轮保留 [PI 接入记录](pthread-pi-mutex-2026-10-01.md) 的语义边界。完整
futex 与 syscall/VFS/I/O 优化总目标仍有后续工作；按用户要求，本轮提交
推送后暂停，不继续开启任务。
