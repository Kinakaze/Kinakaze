# syscall、VFS、内存及 Unix I/O 优化目标

目标持续进行，尚未完成完整 futex/PI/robust pthread 兼容性，也未宣称端到端极限性能。
工作区在开始时已有其他修改，本记录只描述本轮新增的路径。

## 已实现的路径

- 共享 futex 保留域隔离、命名互斥锁、双银行提交和发布前事件唤醒。唤醒及
  requeue 使用稳定线性压缩，避免反复删除队列中间元素；无关地址的 wake、
  requeue、wake-op 直接检查持锁期间的活动银行，避免复制/提交未变化的记录。
- 共享 futex 每次入队探测一个轮转位置，资源用尽时完整清理死亡等待者后才
  返回 ENOMEM。进程/线程创建时间缓存校验当前宿主 PID/TID，fork 子进程清空
  缓存。已取消或已唤醒的等待记录不会在析构时重复获取域锁。
- syscall 汇编桥按未发布 ELF 段复用模板，显式修补三个客体逻辑地址和两个
  重定位继续地址；保留完整寄存器、xstate、嵌套调用、fork 及信号返回处理。
  特定 syscall 返回断点启用时使用原生成器。
- tmpfs 读取按 BTreeMap 范围迭代连续文件页，合并连续 backing slot 的复制，
  只对空洞清零，避免实页先清零再覆盖。继续使用平台优化的内存复制；本轮
  没有增加未经测量的手写 AVX 复制阈值。
- VFS read/write 复用现有原生 pin 查询提供的文件类型，省去单独的预查询，
  覆盖普通文件、设备、管道和 Unix socket 路由。
- raw syscall 273/274 接入 robust-list 注册及查询。普通锁退出恢复处理
  OWNER_DIED、WAITERS、带符号偏移、pending 元素、损坏链表和 2048 项遍历上限。
  native pthread 在客体析构之后、栈及 TID 退休之前调用 libc 的内核清理钩子；
  raw SYS_exit、当前线程的 _exit 和信号终止也清理注册。
  PI 标记的锁只更新 OWNER_DIED；完整 PI 队列转移仍待实现。

robust ABI 对照 [Linux 6.12 futex 系统调用实现](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)
及 [退出清理实现](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/core.c)。

## 对照开关

进程启动前设置为 `0` 使用对照路径；默认启用本轮优化。等待记录的公共退休
处理及 robust 兼容性修正始终启用，不属于开关比较。除了汇编段模板，其余
开关在各所属模块第一次使用时缓存，基准为每个配置启动独立进程。

| 环境变量 | 控制路径 |
| --- | --- |
| KINAKAZE_FUTEX_OPT | 共享队列压缩、空操作剪枝、身份缓存、轮转死亡探测、waitv 批量注册 |
| KINAKAZE_SYSCALL_TEMPLATE | ELF syscall 汇编桥模板复用 |
| KINAKAZE_TMPFS_READ_OPT | 连续页合并及按空洞清零 |
| KINAKAZE_IO_ROUTE_OPT | VFS/Unix I/O 描述符查询剪枝 |

robust 注册/退出恢复属于兼容性修正，不受这些性能开关控制。

## 验证及性能记录

隔离构建目录为 `target/syscall-io-goal`。原生测试统一采用 release 和
`--test-threads=1`；每项并发测试内部仍创建真实线程/子进程。

```powershell
cargo test -p kinakaze-v2-libc -p kinakaze-vfs -p kinakaze-v2-libpthread -p kinakaze-guest-engine --release --locked --no-run --target-dir target/syscall-io-goal
```

`tools/benchmark-futex-queues.py` 支持 futex、waitv、waitv-background、requeue、
tmpfs、syscall 和 route 组件基准。同一固定二进制交替运行开关开/关，
一轮预热、五轮计时，保存二进制及
依赖哈希、逐轮数据和中位数。这些结果不能替代客体 syscall 吞吐或整包安装时间。

首轮 release 构建无警告，改动文件格式检查、`git diff --check` 和基准脚本的
Python 编译检查通过。默认模式总计 1,499 项原生测试通过；26 项忽略项是
子进程助手、独立性能基准或已有的独立运行测试，未计入通过数。

| 模块 | 默认优化 | 全部性能开关关闭 |
| --- | ---: | ---: |
| libc | 557 通过 | 557 通过 |
| VFS | 841 通过 | 841 通过 |
| guest-engine | 60 通过 | 60 通过 |
| pthread | 41 通过 | 不含性能开关，未重复 |

新增验证覆盖普通 robust owner 退出、真实 pthread 返回清理、私有等待者唤醒、
raw 273/274 分发、pending 去重、负偏移、PI 标签的 OWNER_DIED 标记、链表循环、
非法指针、队列 FIFO/bitset/requeue/token 退休、稀疏页及未对齐边界、设备错误
和 Unix 消息边界。原有崩溃事务、域隔离、信号/超时、权限/挂载、共享映射、
SCM_RIGHTS、epoll、寄存器及 syscall 汇编桥执行测试均包含在上述原生套件中。

以下为相同固定二进制中交替开关的组件中位数。futex 竞争项包含 256 条既有
等待记录；tmpfs 项反复读取同一 4 MiB 缓冲区，属于热内存测试。syscall 项只
测桥代码生成，不测调用桥的执行速度。

| 组件工作量 | 对照 | 优化 | 结论 |
| --- | ---: | ---: | --- |
| 共享 futex 空表唤醒 10,000 次 | 2.886 ms | 2.784 ms | 差异很小 |
| 无关地址唤醒 10,000 次 | 13.215 ms | 6.920 ms | 耗时降低 47.6% |
| 入队/唤醒 1,000 次，背景 256 条记录 | 306.979 ms | 13.119 ms | 耗时降低 95.7%，约 23.4 倍 |
| tmpfs 连续页累计读取 1 GiB | 65.117 ms | 31.235 ms | 耗时降低 52.0% |
| 生成 syscall 汇编桥 100,000 次 | 157.604 ms | 1.043 ms | 生成耗时降低 99.3% |
| /dev/null read/write 400,000 次 | 30.386 ms | 21.223 ms | 耗时降低 30.2% |
| Unix 64 B 写读往返 10,000 次 | 67.766 ms | 68.022 ms | 追加 20 轮后仍无已证实的吞吐提升 |

Unix 首轮五次测量中候选为 72.015 ms、对照为 67.714 ms；因这一差异追加
20 轮交替测量，得到上表约 0.4% 的差异。没有据此宣称 Unix I/O 加速。
未运行整套客体发行版或 APT 端到端性能测试。

可复查结果存入 `artifacts/syscall-io-goal-20260930/`：

- `native-tests.json`、`control/native-tests.json` 和四个模块的原生测试日志。
- `futex-benchmark-final.json`、`tmpfs-benchmark.json`、
  `syscall-template-benchmark.json`、`io-route-benchmark.json`、`io-route-repeat.json`。
- `performance-summary.json`、`syscall-audit.json`。
- `run-native-tests.py` 保存本次运行方式；传入 `control` 可重跑关闭开关的套件。

## 仍需完成的工作

1. futex PI LOCK/TRYLOCK/UNLOCK、LOCK_PI2、PI requeue，验证所有权交接、优先级
   继承、链式等待、死锁、信号及超时，而不是接受请求后用普通锁替代。
2. futex_waitv 与 futex2 requeue 已整合，原生及客体探针验证见后续两节；
   继续补充跨进程竞争压力、退出恢复、映射变化及新旧 API 互操作验证。
   PRIVATE_FLAG 与无标记操作须保留 Linux 的不同键标签，不能合并队列。
3. 完整 robust pthread 属性、consistent/not-recoverable 状态、跨进程 robust
   查询权限、所有线程的 exit_group/exec/宿主强制终止恢复和 PID/TID 复用验证。
   本轮 get_robust_list 仅覆盖当前进程线程；现有 pthread robust/PI 属性拒绝仍保留。
4. 统一 mutex/condition 的事件驱动后端后移除 timed mutex 与 condition 的周期
   探测，覆盖取消、信号、隐式解锁和重锁，避免漏唤醒。
5. 分片私有 futex/VFS 热表，Unix 共享数据环及事件驱动背压，保留 SCM_RIGHTS、
   凭据、shutdown、消息边界、epoll 边沿和 fork/exec 行为。
6. 测量 SIMD/AVX2、ERMS/汇编及按大小分派的复制策略；只对确认的热点启用。
   再做客体端到端 syscall、文件、共享映射和 Unix I/O 对照验证。

## waitv 整合与第二轮验证

两版 waitv 合为单一实现：raw 449 只进入
`libs/libc/src/sysadmin/futex_vector.rs`，共享等待组只使用
`libs/libc/src/futex/wait_group.rs`。移除了临时的 `waitv.rs` 与 `vector.rs`，
原测试迁入同一模块。既有 syscall 模板、共享队列优化、tmpfs 连续页和 VFS
路由开关继续共用原构建与基准工具；全部 syscall 的路径清单见
[覆盖记录](all-syscall-path-coverage-2026-09-30.md)。

`KINAKAZE_FUTEX_OPT=0` 逐项注册，默认批量注册。两者共用一个 native 事件、
128 位成员索引、双银行提交、退休、requeue、信号及超时处理。混合向量在
域锁之后获取进程队列锁，先解析全部键、校验值，再发布私有及共享成员。
退休私有成员时只获取一次进程队列锁；原私有单地址等待快路径保持独立。
崩溃 waker 提前发出的事件不会成为成功返回，退出前移除全部兄弟记录。

核对 Linux 6.12 后修正 FUTEX_WAKE_OP 比较编号：3 为 GE、4 为 LE、5 为 GT，
并以旧值为 -2/-1/0 的六种比较验证实际两队列唤醒结果。
PRIVATE_FLAG 和无标记匿名页的键确有不同标签；现有隔离应保留。
依据：[键解析](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/core.c)、
[wake-op 与 waitv](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/waitwake.c)。

第二轮 release 构建无警告；默认模式 **1,523 项通过**，关闭全部优化开关后的
libc、VFS、引擎也全部通过。两种模式使用同一固定二进制，保存 SHA-256。

| 模块 | 默认优化 | 开关全部关闭 |
| --- | ---: | ---: |
| libc | 578 | 578 |
| VFS | 844 | 844 |
| guest-engine | 60 | 60 |
| pthread | 41 | 无开关，未重复 |

新增验证覆盖 128 项、重复地址、私有/无标记混合、后续地址错误优先于前项
值不匹配、全部成员清理、重排队后的原索引、信号处理前退休、SA_RESTART
期限、真实宿主子进程唤醒、域隔离，以及提交前/部分银行/提交后崩溃。
最初关闭开关时，测试过早将首项入队视作全部入队；修正同步条件后完整
套件通过。初始日志保留在 `integration/initial/`，最终记录另存。

`tools/benchmark-futex-queues.py --case waitv` 经过实际 raw 449，测量过期绝对
期限下的完整注册与清理。五轮交替开关、一轮预热、中位数如下；每次校验
ETIMEDOUT 与无剩余记录。这些数字不测线程调度或客体端到端吞吐。

| 工作量 | 对照 | 优化 | 耗时变化 |
| --- | ---: | ---: | --- |
| 私有 8 项，1,000 次 | 22.314 ms | 16.152 ms | 降低 27.6% |
| 无标记 128 项，256 次 | 2,736.952 ms | 83.388 ms | 降低 97.0%，约 32.8 倍 |
| 私有/无标记混合 32 项，1,000 次 | 465.122 ms | 76.068 ms | 降低 83.6%，约 6.1 倍 |

`--case waitv-background` 另外保留 64 条无关的存活等待记录，测量队列负载：
16 项 100 次为 144.462→5.955 ms；128 项 20 次为 415.801→7.627 ms；
私有 128 项 100 次为 84.724→33.974 ms。同一开关还控制死亡探测及身份缓存，
因此结果属于组合优化，不能全归因于批量提交。

最终结果在 `artifacts/syscall-io-goal-20260930/integration/`：

- `native-tests.json`、`control/native-tests.json` 与四个模块日志。
- `waitv-benchmark.json`、`waitv-background-benchmark.json`，含二进制/依赖哈希。
- `coverage.json`、`coverage.csv`：375 个 syscall、3,750 个路径审查项；
  303 个入口已分发、1 个明确拒绝、71 个缺失，完整路径验证仍待逐项完成。
- `run-native-tests.py` 保存本次执行方式。

`FutexVectorProbe.py` 已接入客体兼容性 runner，覆盖共享文件别名及 fork。
本轮最终证据为原生套件和 raw 分发基准，未宣称客体探针或发行版测试通过。
第二轮结束时，PI、futex2 requeue、完整 robust pthread、Unix 共享环和 SIMD
阈值测量仍未完成。futex2 requeue 的后续整合见下一节。

## futex2 requeue 整合与第三轮验证

raw 456 只进入 `libs/libc/src/sysadmin/futex_requeue.rs`。它与传统
REQUEUE/CMP_REQUEUE、WAKE、WAKE_OP 和 waitv 共用
`libs/libc/src/futex/hybrid.rs` 的迁移事务，按域锁、私有队列锁的顺序执行。
每个线程复用一个命名 park 事件；迁移记录只保存数值身份、bitset 和 token，
不向其他进程暴露 Rust 指针。迁移不是唤醒，等待者保持原超时预算，并由
原 token 判断是否被选中或仍需取消。PRIVATE 与无标记键继续分别匹配。

修正两个边界问题：首次私有迁移不得使用表示“未迁移”的零 token；私有
wake、wake-op 和 requeue 在获取队列锁后再次检查桥接提示，关闭提示读取
与迁移发布之间的漏唤醒窗口。命名 park 事件提前置位时，单地址和向量等待
核对已提交记录；若崩溃 waker 尚未提交，继续等待原记录，不退回原地址重建。

原生测试覆盖四种私有/无标记迁移方向、同一 VA 的标签转换、bitset、FIFO、
已有目标等待者、后到私有等待者、waitv 原索引、迁移超时、信号处理前退休、
SA_RESTART，以及提交前、部分银行、提交后崩溃和其他域。新增独立宿主进程
测试确定性地暂停三个私有快路径，在其间发布迁移，并验证新域的首个 token。
私有 wake 和非比较 requeue 只解析键，允许未映射但合法对齐的地址；等待和
比较仍访问值。ABI 对照
[Linux 6.12 系统调用入口](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)
及 [requeue 实现](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/requeue.c)。

最终同一组 release 二进制的默认模式 **1,546 项通过**；关闭四项优化开关
后的 libc、VFS、引擎 **1,505 项通过**。pthread 无相关开关，未重复。

| 模块 | 默认优化 | 开关全部关闭 |
| --- | ---: | ---: |
| libc | 591 | 591 |
| VFS | 854 | 854 |
| guest-engine | 60 | 60 |
| pthread | 41 | 无开关，未重复 |

验证中保留了两类初始失败：构建期间更新了 VFS 的尾部斜线检查和 libc 测试，
旧 DLL 的 `chmod("普通文件/")` 未返回 ENOTDIR；重建后该测试通过。参考
waitv 的重复私有地址测试在杂散中断后会退休旧注册并逐项重注册，观察到
128 条记录不保证随后 wake 仍能选中；诊断日志记录了 SIGCHLD 待处理位
`0x10000`。测试改为等待实际选中，并在断言前 join，避免失败遗留记录。
随后单地址与向量等待统一使用 `signal::interrupt_pending()`，依据 disposition
忽略默认忽略的 SIGCHLD，不再因此退休队列；新增私有/无标记与单地址/向量
四种组合的回归。传统 requeue 负计数及 wake 空 bitset 的错误优先级也在
解析地址之前检查。最终完整默认、对照套件均通过；最终构建期间上述 futex
源码输入哈希保持不变。忽略信号修正前的 1,545 项结果另存于
`before-ignored-signal/`。

结果存入 `artifacts/syscall-io-goal-20260930/requeue/`：

- `native-tests.json`、`control/native-tests.json` 和各模块日志，含固定二进制
  及 SONAME 依赖 SHA-256；`build-binaries.json` 指定实际构建产物。
- `control/registration-turnover.log` 保存忽略信号修正前的诊断及重复地址验证。
- `futex-source/`、`futex-source-sha256.json` 保存最终 futex 源码快照。
- `initial-tests/` 保留初始失败；各次 `build*.log` 保留构建过程。
- `coverage.json`、`coverage.csv` 更新为 375 个 syscall、304 个已分发入口、
  1 个明确拒绝、70 个缺失入口；3,750 个完整路径审查项仍未逐项验证。

本轮没有在并发构建期间采集新的性能结论。上节性能数字对应其固定二进制。
最新完整客体发行目录 `artifacts/syscall-all-paths-20260930/candidate-requeue`
中，两种优化开关配置的 waitv 和 requeue 探针也全部通过，分别记录于
`guest-vector-requeue/report.json` 与 `guest-requeue/report.json`。探针覆盖
128 项混合等待、共享文件的不同 VA 别名，以及 fork 后私有等待迁入共享
文件、外部共享等待迁入当前进程私有队列；报告保存完整发行目录 SHA-256。
这些探针的单次执行时间不作为吞吐结论，也不证明完整 futex 语义全部完成。
PI、完整 robust pthread、Unix 共享环、SIMD 阈值及端到端性能对照仍属于持续目标。
