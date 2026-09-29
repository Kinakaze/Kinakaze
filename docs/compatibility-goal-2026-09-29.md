# 持续兼容性与性能目标

目标状态：进行中。以可复现的行为测试及原生 Linux 对照推进，不以 syscall 分派数量或 `--help` 成功替代完整兼容性。

## 验收口径

- Debian 命令覆盖率：先固定 Debian 版本、架构、软件包清单和版本，再对清单内可执行命令逐项登记。功能通过数除以清单内命令总数为功能覆盖率；未实现测试、缺失依赖、超时和跳过仍计入分母。别名单独报告，并附去重后的统计。99% 是目标，当前未达到或证明。
- syscall/libc：按应用失败补齐参数、错误码、资源生命周期、并发及 fork/exec 行为。静态分派审计只说明入口接线，必须补充真实客体回归。
- 数据库：分别验证 MariaDB 和 Oracle MySQL，覆盖事务提交与回滚、并发客户端、正常重启、崩溃恢复及持久化。MariaDB 通过不能代表 Oracle MySQL 通过。
- 大型软件：逐步覆盖 PostgreSQL、Redis、Nginx、Java、Node、编译器、FFmpeg 等真实工作负载，缺包单独登记，不能计为通过。
- 性能：在同一硬件上固定版本、输入、线程数和存储条件，记录原生 Linux 与兼容层的启动延迟、吞吐、CPU、内存及尾延迟；至少保留预热和多次正式采样。分别报告完整启动、常驻会话和应用内部计时。目标为选定负载中位数不劣于原生 10%，尾延迟不劣于 20%；这只是阶段验收阈值，不是普遍的 native 等价声明。

## 本轮入口与证据

- `python tools/audit-syscalls.py --output artifacts/goal-20260929/syscalls.json`
- 应用基线：`artifacts/goal-20260929/apps-baseline/results.json`；使用 `artifacts/perf-services-20260929/final-v3` 和既有 `app-root`。
- 初始静态审计：264 个分派入口，1 个明确拒绝，110 个未分派；Linux 6.12 x86-64 表中的历史调用也计入。
- `RawEpollCreateProbe.py` 验证旧 `epoll_create` 调用的无效 size、非 CLOEXEC 描述符、eventfd 就绪和消费行为。基线缺少分派，错误码回归失败。
- 本机 `wsl --list --quiet` 显示未安装 WSL；尚未建立原生 Linux 基准。现有 Windows 运行结果不能证明已比肩 native。

## 推进顺序

1. 保留当前工作区改动，建立构建哈希和应用基线。
2. 将已有 libc 能力接入缺失的原始 syscall，并通过客体语义测试。
3. 重跑固定 Debian 清单的功能矩阵，按失败原因完善测试或兼容实现。
4. 补足软件依赖并增加 Oracle MySQL、数据库崩溃恢复测试。
5. 采集启动和运行热点，逐项优化并做前后对照，保持正确性回归通过。
6. 在可用的原生 Linux 环境执行同版本负载，记录剩余差距并持续迭代。

## 第一轮实测结果

构建：`artifacts/goal-20260929/build.log`，Release 原生模块构建、导出检查和格式检查通过；没有执行全 workspace 单元测试。候选包为 `artifacts/goal-20260929/candidate`。构建包含进入本轮前已有改动，不能把版本间全部差异归因于本轮的 epoll 分派修改。

- 原始 epoll 创建探针在旧包失败、候选包通过；候选包的 Python、Nginx、Redis、PostgreSQL、MariaDB 共六项回归通过，见 `candidate-regression/results.json`。
- MariaDB 新增 SIGKILL 后重启并验证已提交行，正常重启和崩溃恢复均通过，见 `mariadb-recovery/results.json` 和候选回归。使用临时数据目录，不操作既有数据库。
- FFmpeg、Java 在应用根目录通过；GCC、Clang、Node、Go 在已有编译环境 `artifacts/performance-20260927/common-root` 通过，见 `compiler-baseline/results.json`。应用根目录缺少的三个编译/运行依赖仍保留为缺包记录。
- 固定 Debian 清单的完整功能基线：758 个可执行路径中 451 通过、61 失败、2 超时、244 未测试，功能覆盖 59.50%。105 个路径为别名；657 个不同目标中 402 个至少有一个路径通过，目标覆盖 61.19%。见 `debian-baseline/functional.json`、`debian-baseline/coverage.json`。这是该固定清单的覆盖率，不是整个 Debian 软件仓库的覆盖率。
- 失败包含 systemd/system bus 未启动、缺少 shadow/wtmp/man 数据、未支持的内核设备/Netlink 接口，以及真实 syscall 缺口，例如 `ionice` 的 `ioprio_get`。它们全部保留为失败；后续分别补充会话场景、必要数据和真实实现，不能通过返回伪造成功提高覆盖率。

所有下述性能路径位于 `artifacts/goal-20260929/`，关闭诊断的配对测量与单独 profiling 分开执行。

| 测量 | 旧包中位数 | 候选中位数 |
| --- | ---: | ---: |
| Bash 内建输出完整启动，10 次正式样本 | 78.762 ms | 77.610 ms |
| 2000 次 select | 5.137 ms | 4.951 ms |
| 2000 次 poll | 6.326 ms | 6.087 ms |
| 2000 次就绪 epoll | 9.189 ms | 9.571 ms |
| 2000 次空 epoll | 6.215 ms | 6.234 ms |
| 2000 次 pipe 创建/读写/关闭 | 100.232 ms | 100.152 ms |
| 2000 次 eventfd 创建/读取/关闭 | 326.373 ms | 324.417 ms |

描述符每项五次正式样本，另有一次预热；所有行为断言通过。报告分别为 `startup/results.json`、`descriptors/results.json`。小幅变化且方向混合，不能据此宣称性能优化成功；启动最大值反而从 86.79 ms 增至 97.72 ms。

三次独立启动诊断的 provider 总初始化中位数为 14.64 ms，运行域启动为 13.03 ms，清理为 4.32 ms；分项存在嵌套，不能直接全部求和。见 `startup-profile/`。eventfd 周期约 162 微秒，代码路径涉及共享 Store、OFD 发布、命名就绪事件和计数序列化，应进一步隔离各项成本。优化必须保留跨进程、dup、SCM_RIGHTS、epoll 和关闭竞态语义。

目标保持进行中：尚未完成 Oracle MySQL 验证、99% 功能覆盖、全面 libc/syscall 语义补齐或原生 Linux 性能对照。

## 第二轮：Oracle MySQL 8.4.11

新增 `tools/test-oracle-mysql.py`，从 Oracle 官方 Debian 仓库获取固定版本的 server-core、client-core、client-plugins，以及 Debian 的 libmecab2。四个包均固定大小和 SHA-256，缓存后支持 `--offline`。工具只解包到独立 `/tmp/oracle-mysql-*` 前缀，不安装到根目录或改动既有 MariaDB。其余动态依赖来自传入的 Debian root。

```powershell
python tools/test-oracle-mysql.py --root artifacts/perf-services-20260929/app-root --dist artifacts/goal-20260929/mysql-candidate --output artifacts/goal-20260929/oracle-mysql-runtime --cache artifacts/goal-20260929/mysql-packages --offline
```

真实装载失败推动了三项修复：

- 添加 binary80 `log1pl`，在接近零时使用 x87 `fyl2xp1` 避免 `1+x` 的消减；保留 long double 的尾数和指数范围，并处理负一极点、负数定义域和 errno。
- 添加 `__getcwd_chk`，先检查调用者声明的目标空间，越界走既有 fortify 终止路径，然后调用实际 getcwd。
- 补充从真实 MySQL ELF 观察到的 `srand48@GLIBC_2.2.5` 版本导出。`tools/abi` 保留导入证据及文件哈希。

`oracle-mysql/abi.json` 的初始审计有三个缺口；`oracle-mysql-after/abi.json` 对 114 个 ELF、1266 项版本需求的原生提供者缺口为零。该审计只证明链接接口存在，不证明全部插件或运行行为通过。

`OracleAbiProbe` 在旧包因缺符号失败，候选包通过。独立 Decimal 参考覆盖多个 binary80 指数/尾数、正负零、最小次正规数、NaN、无穷和定义域错误；检查 fortify 正常读 cwd、ERANGE 和子进程 SIGABRT。既有 `Power80Probe` 同时通过，见 `oracle-abi-final/`。Release 构建和导出/格式检查见 `mysql-package.log`；未执行全 workspace 单元测试。

MySQL 已完成 `--initialize-insecure`、启动就绪、真实 InnoDB 提交/回滚、四客户端并发提交及汇总校验，见 `oracle-mysql-shutdown120/runtime/stdout` 中的 `ORACLE_MYSQL_INITIALIZED` 和 `ORACLE_MYSQL_TRANSACTIONS_OK`。**完整测试仍失败**：正常关机在默认 30 秒内未结束，独立诊断等待 120 秒也未结束；没有把延长超时计为通过。

日志确认连接清理和 Event Scheduler 停止已完成，最后可见 `FTS optimize thread exiting`。参照 MySQL 8.4 分支源码，下一步需要检查 FTS 线程收尾、其完成通知以及后续字典统计线程清理；当前证据不能断言死锁就在某个具体函数。日志开关会引入额外启动停顿，相关结果仅作诊断，最终测试工具已移除这些开关。

为缩小范围增加 `CppFutureProbe.cpp`：真实 libstdc++ 的 shared future、超时等待、detached 线程 promise、`set_value_at_thread_exit`、`std::async` 和 broken promise 行为通过，见 `cpp-futures/results.json`。这排除了该探针范围内的通用故障，不排除 MySQL 特有的并发竞态。

MySQL 的正常重启、崩溃恢复尚未越过关机阻塞点，不能报告通过。后续优先采集非日志方式的线程等待/调用栈证据，再对最小复现修复。NUMA `get_mempolicy`/`mbind` 仍返回未实现，MySQL 明确告警并回退；尚未宣称这些能力已经补齐。

## 第三轮：修复条件变量丢失通知

第二轮的关机阻塞现在已有可复现根因及修复。首先，旧候选包在五次重复中仅两次完整通过，另外三次失败，见 `oracle-stack-repeat/results.json`；单次成功不能证明稳定性。测试工具新增 `--repeat`，保留每轮失败并在全部轮次通过时才报告成功。

新增 `tools/sample-native-stacks.py`，对明确选定的 Windows 进程短暂挂起单个线程、采集寄存器和栈内存后立即恢复，再分析可执行地址候选。它记录进程创建时间并验证线程所属进程；候选地址不是正式回溯栈，可能含旧栈值。实际快照为 `mysql-stacks-77900.json` 和 `mysql-snapshot-21596-*.json`。结合 MySQL ELF 反汇编，主线程的返回地址 `srv_pre_dd_shutdown+0x6ea` 位于等待 purge worker 退出的循环；间隔采样显示主线程仍执行，并非 nanosleep 永久挂住。剩余 worker 在条件变量等待。

根因是 pthread 条件变量为了检查取消而使用 100 ms 的内部超时，但将内部超时完全吞掉：

1. 等待者从原生条件变量队列超时离开，随后阻塞于重新获得互斥锁。
2. 持锁的通知者更新条件并发出通知，此时原生等待队列已没有该等待者。
3. 等待者获得锁后，旧实现因内部超时重新等待，未返回应用检查已经改变的条件。

`pthread_cond_wait` 现在将内部超时作为 POSIX 允许的伪唤醒返回；`pthread_cond_timedwait` 和 `pthread_cond_clockwait` 在绝对截止时间未到时同样返回伪唤醒，真正到期仍返回 ETIMEDOUT。取消检查与重新持有互斥锁的语义保留。

`CondReacquireNotifyProbe.c` 用通知者持锁 250 ms 强制覆盖窗口，测试普通等待、realtime 定时等待、monotonic clockwait，以及真正超时后仍持有互斥锁。旧包在 15 秒外部超时前未完成，见 `cond-reacquire-before/results.json`；修复后与 C++ future 测试一起重复三次全部通过，见 `cond-reacquire-after/results.json`。取消清理、Oracle ABI 和 binary80 power 回归通过，见 `cond-cancel-regression/`。

候选包 `cond-candidate` 的 Oracle MySQL 五轮默认 30 秒关机超时测试全部通过，见 `oracle-cond-fixed/results.json`。共验证十次正常关机、五次重启和五次 SIGKILL 后恢复；每轮重新初始化独立数据目录，并核对提交、回滚、并发行数及总和。

| 应用内部计时 | 本轮中位数 |
| --- | ---: |
| 初始化 | 5843.91 ms |
| 启动至 SQL 就绪 | 1911.28 ms |
| 32 次并发事务 | 4717.62 ms |
| 正常关机，10 个样本 | 1199.68 ms |
| 正常重启至 SQL 就绪 | 1782.84 ms |
| 崩溃恢复至 SQL 就绪 | 2626.98 ms |

正常关机范围 873.35–1478.80 ms。以上是功能测试中的观测，不是 native 对照基准；部分测试与其他回归同时运行。候选构建还包含同期版本变更，不能将所有版本间时间差归因于此修复。

Python、Nginx、Redis、PostgreSQL、MariaDB 和 Java 回归全部通过，见 `cond-app-regression/results.json`。Release 构建与构建时的导出/格式检查通过，见 `cond-build.log`；最终 pthread 单文件格式检查和 Python 语法检查通过。后续全仓格式检查发现同期 `libs/libc/src/fdio.rs` 改动有格式差异，未在本轮修改该文件；没有执行全 workspace 单元测试。

整体目标仍进行中：本轮只证明所列 MySQL 场景与回归通过，NUMA、其他 syscall/libc 缺口、Debian 99% 功能覆盖以及 native 性能对照仍需继续。

## 第四轮：扩展 Debian 功能矩阵

新增十二个命令的真实功能场景：`grops`、`grotty`、`helpztags`、`gettext.sh`、`ipcmk`、`ipcrm`、`scriptreplay`、`iconvconfig`、`dbus-daemon`、`dbus-send`、`dbus-monitor`、`gpgv`。

- 渲染测试先生成 roff 中间格式，再核对 PostScript 标识或终端文本；文档标签、变量替换、转换缓存和终端录制重放都有结果断言。
- D-Bus 使用每个测试目录下的私有 Unix socket，实际调用 ListNames、发送并捕获信号内容，退出时关闭自己的 daemon/monitor。
- 共享内存由独立进程创建，另一个进程检查 IPC_STAT、大小、挂接并写入内容，再由第三个进程重新挂接读取。删除测试先验证对象存在和内容正确，再执行 ipcrm 并验证对象不存在，避免“本来不存在”的假通过。
- gpgv 使用固定公开测试密钥和 Ed25519 签名验证内容，并要求篡改后出现 BAD signature。没有把 gpg 私钥或签名程序作为 Debian 测试依赖；`tests/guest/gpgv-fixture.json` 只包含公开密钥、签名和测试消息，报告记录该文件哈希。

专项十二项全部通过，见 `artifacts/goal-20260929/debian-expanded/functional.json`。随后对相同 758 路径清单重跑完整功能矩阵，首轮结果为 464 通过、60 失败、2 超时、232 未测试。唯一从旧基线通过转为失败的是 lessfile（未返回临时输出路径）；串行复测通过，保留原始 jsonl 失败记录，尚未断言其根因或稳定性已解决。

复测合并后的最新结果为 **465/758，61.35%**；59 失败、2 超时、232 未测试。去重后 657 个目标中 416 个至少有一个路径通过，63.32%。见 `debian-expanded-full/functional.json`。相对第一轮，十二项从未测试转为通过，dig/mdig 也通过；版本差异和测试并发条件不同，不能把所有变化归因于本轮测试新增，更不能将该固定清单视为整个 Debian 软件仓库。

调查同时确认一个需要后续处理的兼容性差异：`ipcs -m -i ID` 报不存在时，对同一个 ID 的 shmctl/shmat/shmdt 实际成功。因此不能把 ipcs 输出直接当作共享内存生命周期证据；其枚举/查询路径需要单独修复。

本轮没有修改 runtime 或性能实现。Python 语法检查和 diff 空白检查通过，整体 Goal 保持进行中。

## 第五轮：真实 SysV 共享内存查询

根据 util-linux 2.38.1 的 `ipc_shm_get_info`，ipcs 首先读取 `/proc/sysvipc/shm`，仅在该文件缺失时尝试 SHM_INFO/SHM_STAT 枚举。因此对已知 ID 的 IPC_STAT 成功，并不能保证 ipcs 能查到它；第四轮的差异来自这一接口缺口。

新增 `/proc/sysvipc` 目录与动态 `shm` 查询。libc 在初始化时将元数据读取函数注册到 VFS；每次打开文件都读取当前 IPC 命名空间的真实共享段，并在对象锁下更新已失效进程的挂接计数。原有键目录现在也登记 IPC_PRIVATE 对象，避免漏掉无键段。

删除语义同时完善了查询可见性：IPC_RMID 立即释放原键；仍有挂接的段保留在查询中并带 SHM_DEST 标志，最后解除挂接后消失。原键可在旧段仍挂接期间创建新的独立 ID。失效登记在查询时清理，普通无挂接删除及当前命名空间的最后解除挂接也会主动清理登记。

这里输出十四个已有权威元数据字段，不编造 RSS 和交换页统计。uid/gid 字段仍沿用现有 SysV provider 的 root-only 元数据限制；本轮没有补齐其所有用户权限语义，也没有实现 SHM_INFO、SHM_STAT 或消息队列。

验证记录均位于 `artifacts/goal-20260929/`：

- `sysv-before/results.json`：旧包因缺少查询目录失败。
- `sysv-after/results.json`：私有段、跨进程 ipcs 查询、挂接计数、延迟删除、IPC 命名空间隔离，三轮全部通过。
- `sysv-key-reuse/results.json`：增加旧段仍挂接时的键复用测试，三轮全部通过。
- `ipc-command-tests/functional.json`：ipcs、lsipc、ipcmk、ipcrm 四项全部通过。原先 ipcs/lsipc 的空输出可通过场景已替换为真实段 ID、大小及枚举断言。
- `ipc-services/results.json`：命名空间基础场景、PostgreSQL、MariaDB 全部通过。
- `ipc-build-retry.log`：Release 原生包构建、导出及格式检查通过。首次构建的一处 usize/pointer 类型错误已修正；没有执行全 workspace 单元测试。

查询支持是兼容性进展，不是吞吐或 native 性能结论。本轮未重跑完整 Debian 矩阵，不能据此修改上一轮完整覆盖率；整体目标继续进行中。

## 第六轮：原始 mincore 与完整回归

`mincore` 的 libc 驻留查询已经使用真实 Windows 工作集，但 syscall 27 尚未分派。本轮将其接入同一个实现，保留负 errno 转换；静态审计变为 266 个分派入口、1 个明确拒绝和 108 个未分派。分派数量不代表完整语义覆盖。

新增 `RawMincoreProbe.py`，同时验证原始 syscall 和 libc：已触碰页面的驻留位、非整页长度向上取整、输出哨兵字节、未对齐地址 EINVAL、无效输出地址 EFAULT、部分 munmap 后的空洞 ENOMEM，以及相邻映射的数据完整性。

测试初稿在部分 munmap 后直接要求邻页驻留，暴露了测试假设不成立：宿主拆分映射后页面可以暂时不驻留。最终测试先实际读取并核对邻页内容，再查询驻留位。保留初始失败报告，不将此项记为运行时修复。旧候选包最终探针在首次 raw mincore 调用返回 ENOSYS；新包全部通过。

本轮报告位于 `artifacts/goal-20260929/continued/`：

- `build-retry.log`：Release 原生包构建、导出和格式检查通过。初次构建受沙箱临时目录权限阻止；重新执行成功。未运行全 workspace 单元测试。
- `mincore-baseline/`：旧包 ENOSYS；`mincore-final/`：原始驻留查询与既有内存策略探针各重复三次，六项全部通过。首次沙箱控制管道失败另保存在 `mincore-before/`。
- `mysql/`：Oracle MySQL 8.4.11 三轮全部通过，覆盖初始化、提交/回滚、并发事务、正常关闭和重启、SIGKILL 后恢复及持久数据检查；每轮完整运行分别为 18.892、18.853、18.609 秒。
- `apps/`：Python、Nginx、Redis、PostgreSQL、MariaDB、SQLite 多进程、FFmpeg、Java 八项全部通过。
- `debian/functional.json`：相同固定清单完整重跑，465/758 通过（61.35%）、59 失败、2 超时、232 未测试，与上轮完整统计一致。`capsh` 和 `tasksel` 超时，`ionice`、`uclampset` 等缺口仍明确保留。
- `startup/results.json`：十次正式样本、两次预热、交替版本顺序，Bash 完整启动中位数旧包 98.115 ms、新包 95.336 ms；最大值分别为 105.844 和 107.141 ms。差异较小且尾部未改善，不据此宣称启动优化成功。
- `startup-profile/`：另行三次诊断，manager-start 中位数 12.709 ms（含 guest-process-create 6.554 ms），providers-total 9.919 ms（含 discover 5.954 ms、bind 1.997 ms），worker-open-runtime 4.814 ms、manager-cleanup 3.899 ms。计时存在嵌套，不能相加；后续优化应优先隔离进程创建和 provider 元数据发现成本。

功能矩阵完成后才执行启动测量，诊断与无诊断测量分开。Python 语法检查和 diff 空白检查通过。本机仍未配置 WSL/原生 Linux 对照环境，以上不能证明已比肩 native；99% 命令覆盖、剩余 syscall/libc 能力和原生性能目标尚未完成。
