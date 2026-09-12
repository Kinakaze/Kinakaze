# 公共进程状态管理

`StateManager` 是与 Windows API 无关的状态机，服务端须在同一把互斥锁下调用它。所有等待者在 ready、commit、abort、disconnect、native exit 后被服务端 Condvar 唤醒；`AwaitActivation`、`AwaitForkReady`、`AwaitExecReady` 和 `AwaitExit` 本身不轮询或阻塞，未就绪时返回 `NotReady`。

`connect(Hello, PeerIdentity)` 返回连接 ID 和 Hello 回复。服务端从 pipe/已打开的宿主进程句柄获得 `host_pid` 和 `birth`，不能相信客户端自报的 PID。一个 worker 只能创建一个客体 Process。Controller 不占客体 PID；当前会话 token 是受信任的会话凭证，服务端还须约束 Controller 的宿主身份。

模块通过 `RegisterModule` 登记全会话一致的 schema，通过 `DefineState` 创建有名字的 `u64` 状态。定义的初值和 fork 策略不可修改。相同定义可重复登记，状态值不会被重置。Define/Read 返回 `Reply::State`，Write 返回 `Reply::Ok`。

fork 状态事务采用 `Prepared → Adopted → Ready → Committed`，提交之前可以 Abort。Prepare 在互斥锁下冻结 Copy 状态、建立 Reset 初始值、引用 Share 对象。256-bit 随机 ticket 只允许一个新宿主 worker 采用；child 只有在 parent commit 后才能访问状态或再次 fork。Prepare 的 request key、commit 和 abort 有明确的重试规则：相同成功请求返回原事务，已 abort 的 key 不能重开，已 commit 的事务不能 abort。已完成的记录保留至 parent 真正退出，并计入配额。

`PrepareForkWithParent` 对应 `CLONE_PARENT`：只能使用调用者已有的 `parent_pid`，全局 PID 1 不允许使用。事务控制权仍属于创建者，逻辑父进程不能代为等待 readiness 或 commit。逻辑父进程在采用之前退出时，预留子进程也会重新归属 parent 0。

exec 使用 `PrepareExec { request_key, replacement_host_pid, replacement_birth }` 预留替换 worker，调用方必须先用固定的宿主进程句柄获得目标 birth。替换进程以普通 Worker Hello 接入时，服务端验证的实际 PID/birth 必须匹配预留身份；它获得原 Linux PID，但提交前只允许 Identity、验证继承模块 schema、MarkReady 和 AwaitActivation。原 worker 的 `AwaitExecReady` 就绪后，`CommitExec` 原子转移 Process 的宿主所有权，保留 PID、父子关系、模块及状态对象。状态不会再次执行 fork 的 Copy/Reset 策略。

提交后的旧连接只可重放自身事务的 CommitExec/AbortExec，不能访问状态或获得 Controller 权限。旧宿主的 EOF 或退出不能撤销新 worker 的事务，也不能释放新 worker 的 Process。提交前任一方 EOF 或真实退出都会撤销 exec；已采用的候选宿主和已替换的旧宿主均保留退出引用，直到匹配 PID/birth 的真实退出通知，避免同一活进程重新注册。每个 Process 只允许一个未完成 exec，目标不得被其他 worker、Controller 或事务占用。

fork 与 exec 分别使用 request key 名字空间，但合计占用每进程 key 配额；所有事务合计占用事务配额。exec 保留 Process 的重试历史，替换后的 runtime 应使用不会与旧实例重复的请求 key。成功 commit 可以在原连接重放，失败或中止记录不可重新采用候选宿主。

Pipe EOF 只调用 `disconnect`：撤销连接、取消未完成事务，但保留已采用的 Process 和对象引用。仅在匹配 PID/creation time 的宿主进程真正退出后调用 `process_exited`；此时释放引用，存活子进程的 parent_pid 置 0。PID 在一个 epoch 内不复用，generation 为 1；分配范围为 `1..=i32::MAX`，与 Linux `pid_t` 一致，耗尽时在变更前拒绝创建或 prepare。完整 Linux PID namespace 和 init/reaper 行为属于后续进程域实现。

Controller 可以用 `AwaitExit { pid }` 等待已接入的 Linux Process 最终退出。服务端在固定的当前宿主句柄真正终止后调用 `process_exited_with_status(peer, status)`，返回的 `Reply::Exit { status }` 保留原生退出码的 32 位位模式，不是 Linux `waitpid` 编码。旧 exec worker、未提交的替换 worker、其他 birth 的退出事件均不能结束当前 Process。EOF 后仍返回 `NotReady`；已知终态可重复读取而不消费。`process_exited` 为旧调用方保留，缺少状态时使用 127。

退出记录默认最多保留 4,096 条，按完成先后淘汰；由 `Limits::max_exit_records` 配置，0 表示不保留。记录不持有 Process 或状态对象引用。不存在、尚未接入或记录已过期的 PID 返回 `NotFound`，超出合法 PID 范围返回 `InvalidRequest`。此控制接口仅供 Controller 使用，不替代客体父子进程的 Linux wait/reap 权限模型。

默认硬上限：2,048 个连接、1,024 个进程（含 prepared 预留）、128 个模块、每进程 4,096 个状态、128 字节状态名、每进程 1,024 个 request key、4,096 个事务、262,144 个对象。`with_limits` 可供启动配置和边界测试缩小配额。状态名不允许空串和控制字符。配额失败发生在变更前。

这里管理的是真实跨进程的模块状态与逻辑进程身份。完整的 Linux fork syscall 还需要客体地址空间、寄存器、信号和线程状态；没有把本状态事务伪装成这些尚未实现的 ABI。
