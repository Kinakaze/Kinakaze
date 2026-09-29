# /proc、syscall、sysfs、tmpfs 验证

本轮以真实客体行为为验收依据；存在分发入口不等于完整实现 Linux ABI。

## 修改与审查结论

- `/proc/<pid>/statm`、`io`、`smaps_rollup`、`auxv`、`fdinfo` 增加目录与元数据支持；`comm` 使用已有进程名称状态。当前进程的 `auxv` 从已重定位 ELF 初始栈发布，包含真实 `AT_PHDR`、`AT_ENTRY`、`AT_RANDOM`、`AT_EXECFN`。
- 修复 tmpfs `fdinfo` 错读普通 FD 表偏移的问题，改为读取 tmpfs 共享描述符位置。
- 原始 syscall 新增 7 个入口：`sendfile(40)`、`fadvise64(221)`、`preadv(295)`、`pwritev(296)`、`renameat2(316)`、`preadv2(327)`、`pwritev2(328)`。位置参数按照 raw ABI 合并，v2 的 flags 从第六个参数读取。
- `posix_fadvise*` 增加无效 FD、O_PATH、管道、负长度和 advice 检查，仍遵循 POSIX 返回正 errno、不设置 errno 的约定。
- tmpfs 增加基于已分配页的 `SEEK_DATA/SEEK_HOLE`，覆盖 EOF、空洞、偏移共享和 unlink 后的 inode 存续。
- sysfs 增加 CPU 的 `core_id`、`physical_package_id`、线程/核心 sibling 列表与掩码，以及 NUMA `cpumap`；数据来自 Windows 拓扑，不以逻辑 CPU 编号猜测物理核心。
- 虚拟文件的 xattr 缺少后端时返回 `EOPNOTSUPP`，先检查路径存在性，避免返回宿主路径转换的 `EINVAL`。
- RTC 设置时间没有可写后端，改为返回 `EOPNOTSUPP`，不再静默成功；不修改 Windows 系统时间。
- `overcommit_memory` 不假装支持 Windows 未实现的 Linux 提交策略；读取 0，拒绝设置 1/2。`/proc/cmdline` 默认空行，不宣称存在 `/dev/sda1` 或 Linux 内核镜像。

审查发现最初的审计脚本漏识别 `SYS_SETXATTR..=SYS_FREMOVEXATTR` 这种符号区间；已修正。xattr 的 12 个原始调用本来就已接入，不计入新增 syscall 数量。

## 验证入口

本地实际结果：55 项 procfs 相关单元测试、10 项 tmpfs/sysfs 相关单元测试、1 项 RTC 测试通过。私有 ConPTY 的客户端输入测试通过。

`artifacts/proc-agent-acceptance-final/report.json` 的最终 `passed=true`：真实 Claude Sonnet 模型完成读取、编辑、四项 unittest 和 diff，测试文件与原始备份保持完整；同时通过 `/proc`、原始文件 syscall、tmpfs/sysfs 边界和真实 Claude CLI 双 Ctrl+C 返回 Bash 的验收。该报告记录构建文件哈希。在线服务未返回标准工具调用的尝试没有计为成功；最终使用逐轮 JSON 命令协议。

`artifacts/proc-agent-common/results.json` 中 shell-spawn、ps、top、vi、python、tmpfs-mapping 六项通过；vim 和依赖 gcc 的 tmpfs-eof 因环境缺少程序而未运行。

当前审计表共 375 个编号：261 个有分发入口，1 个直接拒绝（syslog/EPERM），113 个未在此分发器接入。完整逐项清单为 `artifacts/proc-agent-syscalls.json`，可用下面的命令重新生成。

```powershell
python tools/audit-syscalls.py --output artifacts/proc-agent-syscalls.json
powershell -NoProfile -File tools/build.ps1 -TargetDirectory target/proc-agent -DistDirectory artifacts/proc-agent-dist -NativeOnly -SkipTests
python tools/test-agent-edit.py --root <完整客体rootfs> --dist artifacts/proc-agent-dist --output artifacts/proc-agent-acceptance --offline
```

在线编辑测试通过进程环境提供 `AGENT_API_KEY`，并传入 `--base-url`、`--model`。`--text-tools` 使用每轮一条 JSON 命令的协议，适用于不返回标准 `tool_calls` 的服务。模型生成的工具输出或“已完成”文字不算证据，最终测试和 diff 由宿主再次调用真实客体验证。

`--claude /客体路径/claude` 增加真实 Claude CLI 的双 Ctrl+C 回归。测试在独立样例目录中创建非机密配置，不修改用户 Claude 配置。`tools/test-terminal-input.py` 使用私有 ConPTY 验证自带 Windows 客户端把两次 Ctrl+C 编码为客体输入 `0303`。

## 仍有的边界

- `statm`/`io` 使用承载客体的 Windows worker 计数，包含运行时开销，不是 Linux 内核独立计费。`smaps_rollup` 的 PSS、匿名/脏页等仍是兼容估计，不能用于精确共享内存计费。
- 其他 worker 的 `fdinfo` 尚未发布共享偏移/flags；返回 `EACCES`，不会读取调用者相同编号的 FD。其他进程的 `auxv` 仍是基础兼容字段，不是远程初始栈副本。
- tmpfs 不支持 xattr、打洞/折叠范围等全部 fallocate 模式，扩容也受创建时保留容量限制；已有 inode/共享映射语义不能据此等同完整 Linux tmpfs。
- sysfs 不提供任意硬件设备控制，CPU 拓扑只读，未实现的设备属性不会伪造成功。
- `preadv2/pwritev2` 当前只支持 flags=0，其他 RWF 标志返回 `EOPNOTSUPP`。
- syscall 清单仅审计 `kinakaze_abi_syscall_raw` 对已登记 Linux 6.12 x86-64 LP64 编号的分发，不覆盖所有子命令、标志和装载器特殊处理。剩余项包括 ptrace、userfaultfd、部分 pidfd、io_uring、rseq、NUMA 策略、部分旧 syscall；以生成报告为准。
- Ctrl+C 回归证明当前构建下的原始按键转发、Bash 前台任务中断、Claude 交互界面退出和返回后命令执行。不能据此确认用户正在运行的旧终端实例已更新或其所有运行状态都已覆盖。
