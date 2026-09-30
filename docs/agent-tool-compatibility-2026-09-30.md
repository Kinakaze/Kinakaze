# Agent 工具兼容性与资源回收验证

本轮在 v0.8.0（`8a5b48d`）基础上继续检查真实 agent 工具调用。源码修改尚未发布。使用已安装的 Codex 0.157.1、Claude Code 2.1.283、pi 0.73.1 和本地确定性模型协议服务；CLI 负责实际执行工具，宿主独立检查文件内容、测试结果和取消后的副作用。没有使用在线账号或验证线上模型质量。

## 已复现并修复

| 缺口 | 实际影响 | 修复与回归 |
| --- | --- | --- |
| ioctl 请求未截取低 32 位 | musl 将 `TIOCGPTN` 从有符号 int 扩展到 64 位，Codex PTY 创建返回 ENOTTY | 所有描述符分派前统一按 Linux unsigned int 处理；保留高位垃圾的原始调用探针 |
| 原始 TCGETS/TCSETS 使用 44 字节结构 | Linux x86-64 的 36 字节缓冲之后被写坏 8 字节 | 移除 termios2 的速度字段，按 c_cflag 编解码速度；检查末尾哨兵、输入输出速度与 B0 |
| chdir 保留字面 `/proc/self/fd/N` | 目录描述符关闭或 exec 时 CLOEXEC 后，cwd 不再可恢复，Claude Glob 等操作失败 | 保留实际目录对象；检查关闭、目录重命名、旧路径被替换及 exec 后工作目录 |
| proc-fd 路径 unlink/rmdir 缺失 | Claude 删除任务输出探针返回 EINVAL；tmpfs 目录重命名后按旧路径删除也不可靠 | 先保留 overlay 的删除、copy-up 和 whiteout 分派，再解析普通父目录对象；tmpfs 按 inode 删除；检查 symlink 本身、打开文件、重命名后的诱饵目录、rmdir 和已关闭 fd |
| epoll_pwait/epoll_pwait2 忽略临时信号掩码 | 等待期间应解除阻塞的信号仍然阻塞，无法按预期返回 EINTR | 掩码切换后注册等待并检查 pending；返回时恢复原掩码和 errno；坏指针使用内核复制探测并返回 EFAULT |
| 发布信号时不检查等待线程的掩码 | 临时阻塞的线程定向信号仍错误中断 epoll | 进程、线程和 POSIX timer 的发布只唤醒已登记为接受该信号的 waiter；登记之前到达的信号由 pending 检查接续 |
| 关闭的测试池仍持有输出和宿主句柄 | 长时间重复测试保留池对象时，管道、进程、看门狗句柄和输出缓冲积累 | 关闭输出管道、回收看门狗、释放已 reaped 的 Popen 进程句柄、清空内存缓冲；完整日志仍在磁盘 |

前六项是运行时兼容性修复，最后一项是测试基础设施资源回收。它们都有实际失败或资源增长证据，不把测试脚本协议错误算作运行时缺口。

## 工具覆盖

`tools/test-agent-tool-compatibility.py` 每轮验证以下行为：

- 原生 Read/Write/Edit 或 apply_patch，读取源码、拒绝不匹配的编辑且保持字节不变、修改后重新读取。
- 中文、空格和 emoji 路径及内容；Codex apply_patch 移动文件。
- Claude Glob/Grep、pi grep/find/ls 的真实搜索结果。
- shell 子进程、先失败后成功的三个单元测试、3500 行输出、独立 stderr。
- Codex PTY 交互输入、Ctrl+C、write_stdin，以及中断后的新命令。
- Claude 后台任务输出读取与 TaskStop；pi shell 超时；取消后确认未产生延迟副作用。
- Codex/Claude 的真实本地 stdio MCP：初始化、工具发现、Unicode echo、读取样例源码并检查 SHA-256。Codex 使用真实 tool_search 发现流程。

Codex 和 Claude 各 21 个检查，pi 为 16 个。pi 的 MCP 扩展、在线认证、Gemini 和任意第三方插件不在这份报告范围内。

所有运行按进程树归属隔离。工具矩阵顺序执行，每轮默认设置 16 GiB Job 提交内存上限和 90 秒超时，可通过 `--memory-limit-mb`、`--timeout` 调整。超时关闭本轮 Job，不按进程名称清理其他会话。报告保存每一轮失败、运行库、CLI 和脚本 SHA-256。

pi 使用 Node 22.23.3、ripgrep 和 fd 10.5.0。Debian 自带旧 fd 不支持 pi 使用的 `--no-require-git`，因此将独立 fd 放到测试根的 `/opt/agent-compatibility/bin`；没有覆盖系统工具。此测试根既有的 dpkg 配置错误另行保留，没有将安装命令的失败算作通过。

## 当前证据

原始日志在忽略目录 `artifacts/agent-compatibility-20260930/`。`source-snapshot/` 是 v0.8.0 加本轮六项运行时修复的隔离快照；使用独立 `snapshot-target/` 构建，避免同时进行的其他功能改动混入验证。`epoll-candidate/` 包含临时掩码修复；`wakeup-candidate/` 进一步包含发布侧的掩码检查。

整套回归在 `wakeup-full-checks.log` 抓到一项本轮引入的 overlay 属主回归：proc-fd 删除先转到物理路径，绕过了 overlay 处理，使仍打开的 inode 的 uid/gid 变为 0。已调整 unlink/rmdir 分派顺序；保留这个失败日志，使用 `final-candidate/` 和 `final-full-checks.log` 验证最终实现。

最终验收为 `validation-index.json`：校验本轮六个运行时源码文件与隔离快照的 SHA-256 相同，四份最终报告均使用同一候选包哈希。`final-matrix-repeat-3` 的九轮全部通过，共 174 项工具检查；`features-final` 的 15 项探针各两轮全部通过。`pool-resources-candidate-final` 的八轮关闭和一轮超时回收均保持 164 个宿主句柄，输出缓冲、管道、看门狗、样本句柄均已回收；`agent-resources-candidate-final` 的三次测量均为 269 个句柄、5 个线程和 155897856 字节私有提交内存。

`final-full-checks.log` 中 **2017 项 Rust 测试通过，0 失败，29 项既有忽略**；其中包含 overlay 开放 inode 属主回归和新增的线程/进程/timer 掩码唤醒回归。两组 Python 工具测试各 22 项、原生导出检查、Rust 格式检查和 worker smoke 均通过。smoke 的原生进程、对象、事务退出计数均为 0。整套 Rust 检查针对上述隔离快照，不包含其他会话尚在进行的功能改动。

- `epoll-mask-clean-before` 稳定复现原始 epoll_pwait2 返回 0 而未交付信号；`epoll-mask-after` 两轮通过，包含 libc/raw 两个接口、EINTR、原掩码恢复、EINVAL、EBADF、EFAULT 和成功超时。
- `pool-resources-before` 关闭后仍保留约 2 MiB 输出和两个管道；中间版本分别暴露未释放的 Popen 和看门狗句柄。`pool-resources-after-r3` 刻意保留 8 个关闭池对象，全部缓冲为空、管道关闭、无存活 drainer，句柄稳定为 165；私有提交内存在约 12.1–14.4 MiB 范围。
- `agent-resources-final` 在同一个存活客体中进行 128 次预热和两批各 1024 次 epoll 创建/等待/关闭，穿插 PTY 与 proc-fd 写入删除。预热后及两批结束均为 269 个原生句柄、5 个线程、155979776 字节私有提交内存；客体 fd 数也保持不变。这是本场景无持续增长的证据，不能外推为所有程序无泄漏。
- `proc-remove-final` 六项检查通过，覆盖原生文件系统与 tmpfs 的 proc-fd 删除，以及 cwd/目录创建回归。
- `bun-stress-repeat-5` 的 Bun 文件系统和 32 个并发子进程场景各五轮通过，包含 stdin/stdout/stderr、Unicode cwd 和退出状态。
- `epoll-matrix-repeat-3` 的 Codex/pi 各三轮通过；Claude 的 8 GiB 上限触及后 MCP 工具未能出现，三轮均如实记为失败。使用相同脚本、明确设置 16 GiB 上限的 `claude-epoll-limit16` 三轮全部通过，各 21 项检查；Job 峰值约 11.5 GiB。提高上限用于容纳实际瞬时占用，不代表减少了内存或修复了泄漏。
- `wakeup-matrix-repeat-3` 在最终信号发布修复后，Codex/Claude/pi 各三轮全部通过，共 174 项工具检查，使用同一脚本 SHA-256。包括独立测试验证和 MCP 实际调用记录。Codex Job 峰值约 2.2–2.4 GiB、Claude 约 11.5 GiB、pi 约 0.52 GiB，均按同一 16 GiB 上限顺序运行。
- `features-wakeup-final` 的 15 项 ABI 探针各两轮全部通过，临时阻塞/解除阻塞的线程和进程信号都在覆盖范围内。`pool-resources-wakeup-final` 的八轮关闭和一轮真实超时回收全部通过，关闭后句柄均为 165。`agent-resources-wakeup-final` 的原生句柄和线程均保持 269/5，私有提交内存在第一批增加 4 KiB 后第二批保持不变。

此前 `stable-harness-repeat-3`、`unlink-codex-repeat-5` 和 `remove-matrix-repeat-3` 保留了 Codex 偶发 CreateProcess EINVAL 和 Claude 等待/退出超时。宿主还出现过全局提交内存耗尽，隔离构建日志保留了 LLVM out-of-memory。这些因素尚不能证明是同一个问题，也没有把开启诊断后暂时不复现当成修复证据。

## 复现入口

```powershell
python tools/test-agent-compatibility-matrix.py --root <Codex/Claude 测试根> --pi-root <pi 测试根> --dist <候选包> --output <新目录> --codex <客体 Codex 路径> --claude <客体 Claude 路径> --pi <客体 pi 脚本> --tool-bin /opt/agent-compatibility/bin --mcp --repeat 3
python tools/test-server-features.py --root <测试根> --dist <候选包> --output <新目录> --repeat 2
python tools/test-init-pool-resources.py --root <测试根> --dist <候选包> --output <新目录> --repeat 8
python tools/test-agent-resource-lifetimes.py --root <测试根> --dist <候选包> --output <新目录>
python tools/test-common-workloads.py --root <测试根> --dist <候选包> --output <新目录> --bun <客体 Bun 路径> --only '^bun-(filesystem|subprocess)$' --repeat 5
```

矩阵只在所有计划轮次都通过时标记成功。资源测试刻意保留关闭对象、检查正在存活的客体，避免仅靠程序退出或 Python GC 隐藏回收问题。失败日志和最终报告应一起查看。
