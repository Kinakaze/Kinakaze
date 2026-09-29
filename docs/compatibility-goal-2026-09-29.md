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
