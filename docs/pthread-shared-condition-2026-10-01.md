# 进程共享 pthread 条件变量

共享条件变量已接入共享 mutex、每线程 event/timer、取消和 robust 恢复。`pthread_condattr_setpshared` 支持 PRIVATE/SHARED，新增 getter，并保留 libpthread GLIBC_2.2.5、libc GLIBC_2.2.5/2.34 版本别名。4 字节属性的 bit 0 是共享标志，bit 1 是时钟，设置一项不会覆盖另一项。

48 字节条件对象保存数值身份、时钟和等待计数；共享队列只保存进程、线程、创建时间和等待代次。不同映射地址及 fork 后仍使用同一队列，不向共享内存写入本地指针或 native handle。每线程命名 event 复用现有停车后端，本地取消登记继续接受 `pthread_cancel`。

双缓冲队列先设置 event，再提交选中状态；等待者在命名 gate 下以已提交的 bank 判断是否被 signal/broadcast 选中。进程在提交前、写坏未激活 bank 后或提交后退出均有真实 native subprocess 验证。定向 signal 被取消者消费后转交另一等待者，取消清理前重新获取 mutex；robust 重新获取返回 EOWNERDEAD 时保留拥有权供调用者修复。取消语义参照 [POSIX 条件等待说明](https://man7.org/linux/man-pages/man3/pthread_cond_wait.3p.html)。

`KINAKAZE_PTHREAD_SHARED_CACHE_OPT=1` 默认开启最多 128 项队列 bank 缓存，并剪除空队列 signal 的 native 调用。入队前增加共享计数，移除提交后再减少，因此崩溃只留下多余计数，不会漏掉活跃等待者。关闭开关仍使用相同队列协议，仅每次打开 native 对象。每条件变量最多 512 个排队等待者，资源不足返回 ENOMEM；native 资源不足或故障返回相应错误。死亡线程记录按 native PID、TID 和创建时间清理。

验证基于独立 authoring worktree 的完整 release 构建；使用 Cargo JSON staging 全套 38 个文件后运行真实 Debian guest，避免混用 Rust DLL：

- 同一冻结 native executable 在优化开关 0、1 下各为 59 passed、4 ignored；ignored 项包括性能测量和由父测试主动运行的异常退出 helper。
- 两种时钟、匿名共享映射和同文件不同地址的父子 handoff、4 个子进程 signal/broadcast，以及持锁 signaler 退出后的 robust 修复全部通过。
- 真实 GNU cleanup 帧的共享条件取消、既有 private/robust 取消、cleanup、event timed/signal/fork、robust 和共享 mutex 回归共 14 行全部通过。
- 导出工具 24 项测试通过，包括空 inventory 时保留两个 condattr accessor 的版本。

同一冻结 executable 预热一对，再交替测量 5 对，每次 10,000 个无等待者的共享 signal：关闭开关的中位耗时 12,785.56 ns/次，开启空队列剪枝为 2.00 ns/次。该结果只说明空队列省去了打开、映射和等待 native 对象的调用；极短快路径受计时分辨率影响，不据此宣称竞争队列或应用吞吐改善。[原始测量与二进制 SHA-256](measurements/pthread-shared-condition-2026-10-01.json)。复现：`tools/benchmark-pthread-shared.py --native <frozen> --probe empty-cond --output <report>`；guest runner 选择 `--probe shared-cond`，共享取消使用 `--probe cancel --shared-cancel --robust-cancel`。

这次完成共享条件变量实现及回归。原始 futex PI、PI requeue、调度继承与整体 I/O 优化进度另见对应实现和测量记录，不能用本次 pthread 验证替代。
