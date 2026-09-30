# 文档导航

## 使用与参与

- [上手指南](getting-started.md)：构建环境、客体准备、运行与常见问题。
- [WebUI 工作空间](webui.md)：图形化启动程序、进程管理与本机访问边界。
- [持久会话与托盘](persistent-sessions.md)：单一进程树、可配置 PID 1、终端重连和整树关闭。
- [贡献指南](../CONTRIBUTING.md)：问题报告、开发检查与 Pull Request。
- [发布流程](releasing.md)：版本、源码发行包与二进制验收。
- [安全问题](../SECURITY.md) · [第三方声明](../THIRD_PARTY.md) · [更新记录](../CHANGELOG.md)。

## 架构与开发

- [整体设计](architecture-v2.md) 与 [syscall 清单](architecture-v2-syscalls.csv)。
- [ABI 路线](abi-roadmap.md)、[实现约定](implementation-contract.md) 和 [目标](goal.md)。
- [原生桥接](../crates/bridge/README.md)、[进程管理](../crates/manager/README.md)、[执行引擎](../engine/crates/guest-engine/README.md)。
- [客体依赖准备](../tools/guest-deps/README.md) 与 [CUDA provider](../libs/libcuda/README.md)。

## 兼容性与性能

- [深度优化构建与短程实测](deep-optimization-measurements-2026-09-30.md)：Release、73 项原生测试、三项客体检查与两轮交错测速，缺失路径查询耗时降低约 67%。
- [APT 启动与账户查询热点](apt-startup-account-hotpaths-2026-09-30.md)：按安装阶段归因、PE 导出索引复用、ELF reader 复用与账户查询分配缩减。
- [APT 热点优化运行验收](apt-hotpaths-validation-2026-09-30.md)：102 项原生/缓存测试、账户与进程资源回收、357 包完整安装及性能判断边界。
- [Node/npm 日志深度分析](file-io-deep-analysis-2026-09-30.md)：完整路径统计、管道等待重叠、重复模块发现、小文件成本与 debconf 预配置失败。
- [NTFS 与 init 共享镜像优化](vfs-ntfs-shared-cache-2026-09-30.md)：不存在路径复用、EA 直接解码、小镜像共享与有界缓存资源。
- [utmp 文件锁与定位修复](utmp-locks-2026-09-30.md)：OFD/进程锁、tmpfs utmp、共享位置与并发记录更新回归。
- [文件 I/O 诊断与定向优化](file-io-optimization-2026-09-30.md)：默认双 worker，1,005 万条事件，chmod 后数据回写退化、元数据句柄复用与描述符生命周期。

- [APT 与开发工具第三轮验证](apt-tools-performance-2026-09-30.md)：inode 锁快路径、离线链接接口、真实包安装与 Git/构建工具回归。
- [Node/npm 完整安装性能](node-install-performance-2026-09-30.md)：357 个包的真实安装计时、SIMD fork 复制、原子创建元数据与资源回收验证。
- [APT 本地安装第二轮优化](apt-local-performance-2026-09-30.md)：重复元数据查询消除、交错解包基准与真实 Node/npm 离线安装。
- [Bash setpgid 修复](bash-job-control-2026-09-30.md)：短命令管道的进程组生命周期、前后台作业与 Ctrl-C 回归。
- [Agent 场景与本地 APT 安装](agent-apt-validation-2026-09-30.md)：八类应用回归、zcode 状态恢复修复、APT 安装/重装分阶段对比。
- [Syscall、ptrace 与执行热路径](syscall-ptrace-performance-2026-09-30.md)：Windows 调试后端、JIT/AOT 陷阱缓存、汇编检查、SIMD 分派、资源回收验证和剩余兼容范围。
- [Agent 与开发工具链](agent-toolchains-2026-09-30.md)：Codex/Claude/pi 工具闭环、长 argv、默认编译链接接口与性能对照。
- [Agent 工具兼容性与资源回收](agent-tool-compatibility-2026-09-30.md)：PTY、原始 termios、proc-fd 删除与工作目录、epoll 信号掩码、真实工具矩阵和资源回收回归。
- [Agent 运行时修复与验证](agent-runtime-fixes-2026-09-30.md)：dsh 内存探测、Bun 信号等待、稀疏内存丢弃、OpenCode/zcode 真实工具回归。

- [集成验证记录](validation.md) 与 [开发进度](progress.md)。
- [默认启动、数据库与 io_uring](server-performance-2026-09-29.md)：udev 默认策略、常驻 Python、内存策略、MariaDB/AstrBot 运行结果和剩余边界。
- [关闭响应与国内源验收](shutdown-mirror-validation-2026-09-27.md)：托盘退出延迟、整树回收及中科大镜像直连安装。
- [软件运行边界](software-boundaries-2026-09-22.md) 与 [浏览器启动](browser-startup-2026-09-22.md)。
- [Minecraft 渲染](minecraft-rendering-2026-09-23.md)、[GNOME](gnome.md) 与 [网易云音乐](netease-cloud-music-2026-09-22.md)。
- [启动性能基线](startup-performance-2026-09-22.md)、[第二轮](startup-performance-round2-2026-09-22.md)、[第三轮](startup-performance-round3-2026-09-22.md)、[第四轮](startup-performance-round4-2026-09-23.md)、[第五轮](startup-performance-round5-2026-09-23.md)、[第六轮](startup-performance-round6-2026-09-23.md)。
- [跨进程共享 futex](futex-shared-2026-09-23.md)。

带日期的记录描述当时的源码、产物与测试环境，其中的本机路径和历史命令用于追溯，可能不再适用于当前版本。首次构建请以 [上手指南](getting-started.md) 为入口；忽略目录 `artifacts/` 中的原始日志需要本地重新生成。
