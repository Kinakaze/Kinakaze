# Syscall、ptrace 与执行热路径（2026-09-30）

本轮实现 Windows x64 原生调试后端，并加速现有 JIT/AOT syscall 桥接和两字节陷阱回退。下列结果来自本机原生测试和微基准；尚未完成全部 Linux syscall，也没有完成完整的 Linux GDB/strace 兼容验收。

## 已落地的性能改动

- 未跟踪调用以内联汇编读取 Windows x64 `TEB -> PEB -> BeingDebugged`。普通路径无需 ptrace TLS、锁、分配或 Debug API 调用；每次读取仍能发现后续 attachment。寄存器跟踪帧放在独立 cold 函数中，现有生成代码继续调用同一 syscall gate。
- JIT/AOT 两字节 syscall/GS 陷阱查找加入 16,384 槽的固定缓存，最多探测 32 槽。命中无需锁；冲突溢出仍查权威 BTreeMap。记录版本校验防止槽位删除、复用时读到另一地址的目标；卸载和 fork 恢复同步失效缓存。缓存固定占用 384 KiB，不持有可执行页所有权。
- `getpid` 缓存一个逻辑 PID/命名空间 PID 对。逻辑身份变化会使旧值失效，clone 进入新的 PID 命名空间时显式失效；`getppid` 继续查询当前父进程，以保留 reparent 行为。
- 默认关闭的 syscall 日志不再每次查询环境变量。命名空间日志目录仅初始化一次，post-wait 日志先检查原子开关。
- 私有匿名 `mremap` 直接复制本进程已固定的 VMA，避免 `ReadProcessMemory` 内核往返。已有可读源页不再临时修改保护。复制使用 AVX2 或编译器/CRT 的 SIMD、REP MOVSB 路径；手写 AVX2 仅用于 768 KiB–1 MiB 范围，其他大小保留本机优化的复制实现。512 KiB 和 2 MiB 的实测显示扩大 AVX2 使用范围会变慢。

PEB 布局属于 Windows x64 平台假设；布局变化需要重新核对。[Microsoft PEB 文档](https://learn.microsoft.com/en-us/windows/win32/api/winternl/ns-winternl-peb)

## 原生 ptrace 后端和资源所有权

每个被跟踪进程由一个专用线程执行 `DebugActiveProcess`、`WaitForDebugEventEx`、`ContinueDebugEvent` 和 detach。运行期间阻塞等待原生事件，停止期间阻塞等待命令，无忙轮询。每个 tracee 使用一个 4 KiB 控制页。

已覆盖 ATTACH、TRACEME、SEIZE、INTERRUPT、CONT、SYSCALL、SINGLESTEP、DETACH、KILL，内存 PEEK/POKE，普通寄存器、调试寄存器、x87/XMM 寄存器以及 PRSTATUS/FPREGSET。syscall entry/exit 报告允许修改号码、参数和结果，包括错误返回和未知号码。GET_SYSCALL_INFO 从当前寄存器生成，避免 SETREGS 后返回过期内容。原生 CONTEXT 使用 16 字节对齐的缓冲区。

process handle 和控制页由 RAII 回收；CREATE_PROCESS/LOAD_DLL 事件中的文件句柄显式关闭。detach 清除会话与报告，进程退出释放会话，即使调用者尚未消费退出状态。每个目标只保留当前可等待报告，避免不读取停止状态而不断恢复时累积历史。测试 fixture 在断言失败时也会终止并回收自己创建的 child 和 event。

`memfd_create` 的 tmpfs 分支在发布 fd 前 unlink，原生分支使用已有的 unnamed/delete-on-close 文件能力。dup 保留同一内容，最后一个 fd 关闭后回收资源。

## 测量方法与结果

环境为 Windows 11 10.0.26200、Intel Family 6 Model 186、rustc 1.95.0。独立热路径基准直接编译生产模块，使用 `opt-level=3`、预热和 5 次采样中位数；将基准线程固定在一个可用 CPU，避免混合架构处理器的核迁移影响比较。复制缓冲区按 4096 字节对齐，匹配 VMA 搬迁。JSON 保留输入源码 SHA-256、迭代数、最小值和最大值。

| 测量对象 | 对照 | 加速路径 | 单位 |
| --- | ---: | ---: | --- |
| 调试状态检查 | 1.719 | 0.249 | ns/次 |
| 1,024 个陷阱地址循环查找 | 21.497 | 1.899 | ns/次 |
| 1 MiB 复制 | 38.848 | 38.609 | μs/次 |

数值仅比较该次运行的热路径，不能外推为整个程序或启用 ptrace 后的吞吐倍率。SIMD 收益受 CPU、缓存和复制大小影响，分派边界可通过同一工具重新测量。

另外，最新 release libc 测试中的真实 ABI 调用测得 `getpid` 约 2.06 ns、raw getpid 约 9.84 ns、raw gettid 约 7.79 ns、raw 未知号码约 4.37 ns。此前同一测试中未加入 PID 缓存时，libc/raw getpid 分别约 782.18/797.57 ns；主要收益来自消除重复共享 PID 表查询。这个对照已经包含本轮的汇编 guard，不能理解为相对于整个仓库原始版本的综合加速比。

原始结果：`artifacts/syscall-fastpaths.json`、`artifacts/syscall-abi-benchmark.json`。产物目录不进入版本控制，需在本机重新运行。

## 验证和复现

完整 debug suite：libc 545 项通过、5 项忽略；guest-engine 59 项通过、1 项忽略。release 专项 suite 共 88 项通过。

验证包含：真实 Windows child 的 attach/停止/继续/退出，native SINGLESTEP、SEIZE/INTERRUPT、停止期间 KILL，syscall 修改和 PEEK 的 errno=-1 特例，64 次 attach/detach 的 handle 稳定性，10,000 次未消费停止报告的有界存储，64 次 memfd/dup/close 的生命周期，以及陷阱缓存并发槽位复用和溢出回退。生成的 syscall trampoline 单独验证参数、普通寄存器和扩展状态保存。它们尚不等价于在完整 guest 程序上运行 GDB/strace。

```powershell
python tools/test-syscall-ptrace.py --suite all --output artifacts/syscall-ptrace-full-tests.json
python tools/test-syscall-ptrace.py --profile release --output artifacts/syscall-ptrace-release-tests.json
python tools/test-syscall-ptrace.py --profile release --benchmark --output artifacts/syscall-abi-benchmark.json
python tools/benchmark-syscall-fastpaths.py --output artifacts/syscall-fastpaths.json
python tools/audit-syscalls.py --output artifacts/syscall-coverage.json
```

测试脚本使用隔离 Cargo target 和该次构建的确切 DLL 产物进行 SONAME staging，避免与其他并行构建混用。临时 staging 自动删除；测试/构建超时时按自己持有的 PID 终止该进程树。

## 仍需实现的兼容范围

对 Linux 6.12 x86-64 表的最新静态审计：375 个号码中，300 个已有分派、1 个明确拒绝、74 个缺少分派。已有分派也包含限制实现和既有 stub，不能据此声明 Linux syscall 已全部实现。先前增加的分派包括 brk、mremap、itimer、semtimedop、pselect6、getcpu 等 32 项；随后补充 futex_wait 和 futex_wake。`semtimedop` 提供 raw 220 和无版本 libc 导出，版本化 ELF 导入还需增加经观测的 ABI 证据。

当前 ptrace 以原生进程 attachment 为单位，未完成 Linux 每 TID 独立跟踪、fork/clone/exec 关系转移、完整 signal-delivery/group-stop、NT_X86_XSTATE，以及混合 traced/untraced child 的全部 wait 语义。尚未实现的 fork/exec event 选项会明确拒绝。存在本地 ptrace 状态时，按进程组选择 wait 暂返回 EOPNOTSUPP；目前仅支持正 PID 和 -1，避免调试目标持有共享进程表锁时 tracer 查询该表而死锁。进程组选择需要独立于目标可运行状态的成员数据。[Linux ptrace 语义](https://man7.org/linux/man-pages/man2/ptrace.2.html)

`mremap` 当前服务单个完整私有匿名 VMA；共享/文件映射移动和 DONTUNMAP 尚未实现，原地增长后的相邻 VMA 合并、mapping advice 迁移仍需补齐。完整 memfd sealing 也尚未完成。

## 后续补充：安装阶段和等待 ABI

`pselect6` 现在安全读取 timeout/signal-mask，检查 fd 位图，并写回剩余时间；libc `pselect` 仍保留输入 timespec。新增 raw 454/455 复用既有 futex 等待队列和共享内存后端，校验 U32 大小、掩码、flags 和绝对时钟。新式 wake 的零数量返回 0，不沿用旧式 wake 的至少唤醒一个行为；Linux 6.12 本身只接受 U32，NUMA 位不在有效 flags 中。[Linux futex syscall 实现](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/syscalls.c)、[flags 定义及校验](https://github.com/torvalds/linux/blob/v6.12/kernel/futex/futex.h)

文件创建将 mode/uid/gid 合并为一次 inode EA 更新；chmod/chown 复用已经打开的独立元数据句柄及 inode 锁，减少重复 reopen 和递归命名锁操作。继续保留 Windows 属性投影、并发串行化、xattr、setgid 继承和 fsync/fdatasync 的持久化屏障。64 次交替只读/可写初始化测试验证内容、所有权、xattr 和句柄回收。

`tools/benchmark-dpkg-phases.py` 生成确定性本地 deb，在 guest 的唯一 `var/tmp` 子目录中使用私有 dpkg 数据库、安装根、日志和 APT_CONFIG。分别计时 unpack、configure，并可通过真实 apt-get 安装相同 deb。配置脚本只写 DPKG_ROOT，另外启动若干 `/bin/true` 模拟脚本中的子进程。计时后逐个校验文件内容和配置标记；成功后在关闭所属 Job 后删除该 fixture，失败时保留路径供排查。没有禁用 fsync，也没有使用 force-unsafe-io。

当前候选构建同时包含工作区已有的 sync_file_range 数据回写实现；整体安装对比包含该改动，不能把所有收益单独归因于元数据合并。fork 诊断显示配置脚本的主要剩余成本包括 native worker 创建、fork 快照复制、恢复握手和 exec 加载；本次不声称这些阶段已经显著提速。

最终使用 256 个 4 KiB 文件及 12 次 `/bin/true` 配置脚本，按 baseline/candidate/candidate/baseline 顺序运行，每个 session 两次，各版本四个样本。每次从私有空数据库开始，包含真实 fsync；不包含下载耗时。以下为本机暖缓存实测，不是所有包的加速保证。

| 阶段 | 基线中位数 | 候选中位数 | 观察到的耗时下降 |
| --- | ---: | ---: | ---: |
| dpkg 解包 | 4.318 s | 3.733 s | 13.6% |
| dpkg 配置 | 1.092 s | 1.011 s | 7.4%，样本区间明显重叠 |
| apt-get 本地安装全流程 | 6.296 s | 5.560 s | 11.7% |

配置仍约 1 秒，样本波动不足以支持稳定提速结论。最终比较 JSON 位于 `artifacts/apt-syscall-20260930/comparison.json`，四组原始 report 保存每阶段 wall/CPU 时间和 distribution SHA-256。最初的单次冒烟结果不参与该表。

后续 release 专项回归 **104 项通过**，覆盖新增 futex/pselect ABI、原生 ptrace、映射、SYSV IPC、指令桥接、inode 更新和 writeback；未运行本轮完整 debug suite。实际 Debian guest 的 `SyscallWaitProbe.py` 验证跨线程掩码唤醒、零数量唤醒、非法参数、绝对超时及 pselect6 写回，结果通过。原始记录为 `native-tests-final.json` 和 `guest-waits-final/report.json`。

```powershell
python tools/benchmark-dpkg-phases.py --root artifacts/goal-systemd-idle/debian-root --dist artifacts/apt-syscall-20260930/candidate --output artifacts/dpkg-phases-new --repeat 3 --files 256 --children 12 --apt --io-probe
python tools/test-syscall-waits.py --root artifacts/goal-systemd-idle/debian-root --dist artifacts/apt-syscall-20260930/candidate --output artifacts/guest-waits-new
```
