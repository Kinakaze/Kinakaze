# 更新记录

## 0.9.0 — 2026-09-30

- 修复 Bun/Node agent 的信号嵌套、条件变量等待、ELF 私有映射 futex、终端 ioctl 和 proc 描述符路径问题，扩展 dsh、OpenCode、zcode、Codex、Claude 和 pi 的功能回归。
- 修复 msync 对堆、栈和 ELF 映射的校验；匿名页丢弃不再触碰整个稀疏区间。大块匿名分配纳入进程 Job 提交量限制，内存不足返回 ENOMEM；补充内存、句柄、线程及子进程回收检查。
- 增加私有匿名 mremap、semtimedop 和原生调试事件驱动的 ptrace 路径，修正 memfd 的匿名文件生命周期及部分原始 syscall 分派。
- 改进 AOT 间接跳转目标识别并升级缓存格式；增加容量固定的 trap 查询缓存，避免缓存随运行时间无界增长。
- ptrace 的 fork/exec 事件选项以及共享/文件映射 mremap 仍不支持。Agent 验证使用本地确定性模型接口，不能代表在线模型服务或完整 Linux ABI 兼容性。

## 0.8.0 — 2026-09-30

- 修复 Claude 文件工具使用 proc 目录描述符路径时的创建、打开、元数据和原子替换问题。已关闭的描述符、`O_NOFOLLOW`、目录重命名和 tmpfs 保留 inode 均按客体路径处理。
- 通过现有共享交接传递 `execve` / `posix_spawn` 的 argv，不再把长参数写进 Windows 命令行。Codex 约 38,500 字符的 shell 准备命令不再间歇返回 `ENAMETOOLONG`。当前进程的 `/proc/self/cmdline` 使用初始 argv；其他进程的长 cmdline 仍受共享进程表字段限制。
- 首次安装在 Debian 多架构目录提供 ELF 链接接口，GCC、Clang 和 Cargo 不再把原生 PE 库当作链接输入。补齐 CMake 需要的 `__wmemcpy_chk@GLIBC_2.4`。
- 减少 provider 发现的临时导出列表和重复区段查询，并复用目录枚举已有的文件类型。同机启动中位数约有小幅下降，最慢样本和部分尾延迟没有改善。
- 不宣称所有 agent、语言版本或 Debian 命令均已兼容，也没有同硬件原生 Linux 对照。



## 0.7.0 — 2026-09-29

- 修复 pthread 条件变量内部超时后重新获取互斥锁时可能丢失通知的问题；Oracle MySQL 8.4.11 的事务、正常关机、重启和崩溃恢复重复测试通过。
- 增加真实 `/proc/sysvipc/shm` 查询，支持 IPC_PRIVATE 段枚举、跨进程挂接计数、延迟删除和删除后的键复用，完善 ipcs/lsipc 功能验证。
- PTY 原生就绪事件接入混合 TCP/PTY 的 poll/epoll 等待，降低终端会话空闲轮询；补充数据、挂断和重置回归。
- 接通原始 `mincore` syscall，验证真实驻留位、输出边界、无效地址和部分解除映射后的空洞。
- 扩展 Debian 命令功能场景；固定清单完整复测 465/758 通过（61.35%）。MySQL 三轮及 Python、Nginx、Redis、PostgreSQL、MariaDB、SQLite、FFmpeg、Java 回归通过。
- 尚未达到 Debian 99% 命令覆盖，仍有 syscall/libc 和内核设备能力缺口；没有原生 Linux 性能对照，不宣称已比肩 native。

## 0.6.0 — 2026-09-29

- 会话服务改用 socketpair 接收信号唤醒，避免和终端 poll 混在同一集合时被迫周期回扫。默认不再启动客体 udev：相关单元要存在 `/etc/kinakaze/enable-udev` 才会运行，设备仍由兼容层初始化。
- 接入 Linux `io_uring` 的 setup、enter、register 和 SQ/CQ 映射，支持 nop、读写、fsync 以及按 user data 取消。只读兴趣的 epoll 可以等待 ring 完成事件。旧编号 `epoll_create` 也可分派。
- signalfd 在一次等待内复用共享掩码映射，并读取当前发布值。inotify 对未变化的空原生监视集只检查队列头，不再每次重编码监视表。
- 动态链接器按客体路径解析 `$ORIGIN/..`，Python wheel 里常见的兄弟库路径不再被 Windows 字面路径丢掉。只写打开的文件仍可 `fstat` 到 inode。新线程继承创建者的阻塞信号掩码。
- 补齐 `log1pl`、`__getcwd_chk`、`srand48@GLIBC_2.2.5`、`mincore`，以及 `feenableexcept` / `fedisableexcept` / `fegetexcept`。`madvise` 支持转储排除和匿名页 `MADV_DONTNEED`；`MADV_FREE` 仍返回 `EINVAL`。基础包保留 ucf 模板和虚拟 Provides。
- Oracle MySQL 8.4 可以完成初始化、事务提交/回滚和并发客户端，但正常关机仍会停住，重启和崩溃恢复不能计为通过。固定 Debian 清单的功能覆盖率仍约 59%。不宣称完整 Linux ABI，也不宣称性能已比肩原生 Linux。

## 0.5.0 — 2026-09-29

- 修复并发回收子进程时的等待错误：每次等待持有自己的句柄引用；已消失或已被其他等待者取走的退出记录会重新扫描，不再误报完整性错误。过期快照打开的句柄不再写回 fork 登记表，避免槽位耗尽后无关的 `fork` 返回 `EAGAIN`。
- 补齐当前进程的 `/proc/<pid>/statm`、`io`、`smaps_rollup`、`auxv`、`fdinfo` 与 `comm`。`auxv` 从已重定位的 ELF 初始栈发布。其他进程没有共享偏移时，`fdinfo` 返回 `EACCES`。
- sysfs 公布来自 Windows 拓扑的 CPU `core_id`、`physical_package_id`、sibling 列表与掩码，以及 NUMA `cpumap`。虚拟文件缺少 xattr 后端时返回 `EOPNOTSUPP`；设置 RTC 和未实现的 overcommit 策略不再伪造成功。`/proc/cmdline` 默认为空行。
- 原始 syscall 增加 `sendfile`、`fadvise64`、`preadv`、`pwritev`、`renameat2`、`preadv2`、`pwritev2`。tmpfs 按已分配页支持 `SEEK_DATA` / `SEEK_HOLE`。
- 降低日常路径开销：原生镜像导出名改为有界搜索；内容未变的 epoll 关闭、重绑和空闲扫描不再重写共享集合。已有完整原生通知的 Unix 只读、非 ET epoll 不再做 10 ms 周期回扫。单次等待内复用 mountinfo、timerfd 与 inotify 的共享映射，权威状态仍由 init 持有。
- systemd 空闲时仍有明显单核占用；timerfd、signalfd、inotify 的完整事件通知尚未补齐。不宣称完整 Linux ABI。

## 0.4.0 — 2026-09-28

- 修复托盘“回到终端”打开的窗口一片空白、输入无回显：常驻 init 的标准句柄指向日志与空设备，终端客户端不再继承它们，改为使用新控制台自身的输入输出。
- 修复 `apt install curl`、`wget` 等已预装工具时 dpkg 报错“trying to overwrite ..., which is also in package kinakaze-base”：manifest 新增 `/etc/apt/preferences.d/kinakaze-base`，APT 改为提示 `kinakaze-base` 已是最新版本；构建时校验该名单与基础包 Provides 一致。
- 运行时升级后，rootfs 中重新构建的 `ld-linux-x86-64.so.2` 解释器仍可直接执行：按内部 DLL 名与原生命令导出识别，不再依赖构建时间戳；缺少导出或截断的镜像仍被拒绝。

## 0.3.0 — 2026-09-28

- 将跨进程资源、挂载与命名空间等共享状态集中交给 init 保管，补齐进程退出、句柄复制、fork/exec、监听 socket 和排队消息的生命周期处理。
- 实现 tmpfs 共享 mmap、稀疏及超出文件末尾的映射；跨进程截断会撤销失效页面并产生 SIGBUS，重新增长后按 inode 状态恢复，保留未截断的私有脏页。
- 补齐映射在 fork、关闭描述符、删除文件、局部替换及配额耗尽时的生命周期；处理延迟映射作为文件和管道 I/O 缓冲区，并让父进程正确取得默认 SIGBUS 的信号退出状态。
- 修复挂载移动、传播、根目录切换、proc 文件绑定及保留目录句柄的元数据查询，恢复 journald、udev 与 tmpfiles 标准服务及 systemd 服务凭据挂载。
- 补齐 libc、信号、线程、目录枚举、权限与账户接口的边界行为，扩展真实 Debian 命令、服务生命周期、数据库持久化和编译器回归。
- 保留精简的首次联网安装运行包：EXE、依赖 DLL、Debian manifest 与必要资源；宿主无需 Python，许可材料统一放在 `licenses/`。
- 回归覆盖见实际测试结果；不宣称所有 Debian 程序或完整 Linux ABI 均已兼容。

## 0.2.0 — 2026-09-27

- Windows 运行包携带 EXE、依赖 DLL 和 Debian 清单，首次启动自动联网安装，无需宿主 Python；默认使用中科大镜像，支持系统代理、清单配置与环境变量覆盖，实时显示下载和安装进度。
- 默认 Debian bookworm 环境包含 306 个锁定软件包；内置 OpenSSH 服务，默认监听 `127.0.0.1:2222`，账号 `root`、密码 `kinakaze`。
- 同一 rootfs 共用一个常驻进程树，客体 PID 1、启动流程与关机命令由清单指定；默认配置由 systemd 启动 SSH 和会话服务，支持自定义 init。
- 增加托盘管理、可断开重连的 Bash/ELF 交互终端与 WebUI 入口；修复鼠标控制序列污染终端输入，缩短关闭进程树时的等待。
- 改进原生镜像加载与启动性能，补齐 libc 正则、文件系统、socket、定时器、信号与 X11 行为，并扩充回归验证。
- 修复默认忽略的 SIGCHLD 导致异步 I/O、文件刷新与写时复制误报 EINTR；发布构建迁移至支持原生 IoRing API 的 Windows Server 2025。
- 精简运行包布局，移除说明文档、构建/验证报告和重复 CMD 入口；第三方许可集中保留在 `licenses/`。

- 默认清单预制 APT/dpkg、签名源、密钥、证书、基础命令和 dpkg 文件所有权；增加离线 setup、默认登录 shell 和 Windows 启动入口，原子初始化恢复 Linux 权限。
- 补齐宽字符 printf 文件流和 checked/va_list 入口，修复 Debian hello 的 `__wprintf_chk@GLIBC_2.4` 装载缺口，增加真实 ELF Unicode、浮点与可变参数回归。

- WebUI 增加图形化程序启动、独立参数编辑、cwd/环境变量设置、进程详情和确认结束操作，区分待命环境与应用进程。
- 重做桌面与移动布局，增加搜索筛选、资源排序、采样暂停、断线恢复和深浅主题；变更接口校验同源令牌，结束操作核对完整进程身份。
- 增加真实运行时的浏览器回归脚本与 [WebUI 使用说明](docs/webui.md)。

- 默认安装 Debian bookworm 的 Essential/required/important/standard 软件及依赖和推荐，共 306 个锁定包；完整保留文件、链接、权限与大小写，支持 Bash 登录、补全、man、编辑器和 Python/Perl。
- 补齐数据库枚举、可变参数输出、COPY 数据与 4 字节自旋锁 ABI，以及 getent 所需 NSS、aliases、gshadow、ethers 入口；修复 netlink 缓冲区选项和内核默认目的地址，补齐跨进程 cwd/root 链接。
- 首次安装使用有界并发校验与写入，增加真实 Debian 命令行和 ELF 回归。详见 [Debian 验证记录](docs/debian-standard-validation.md) 和 [v0.2.0 发布说明](docs/releases/v0.2.0.md)。

## 0.1.0 — 2026-09-26

- 增加可选 rootfs 清单，外部传入优先，仅初始化不存在或空的 rootfs；完整预检、源文件 SHA-256、并发锁与暂存目录提交避免留下半成品。
- 增加长驻 init 的会话文件和 `init launch --parent PID`，支持新客户端向已有进程树启动新进程、设置 cwd/环境并等待退出。
- 启动凭据与 worker 凭据分离，会话文件限制为当前用户，客户端验证宿主 PID、创建时间和管道对端。
- 基础发行 rootfs 与初始化资源从已锁定的 BusyBox/netbase 包生成，不依赖旧工程目录；附带源码、版权材料和 SHA-256。
- Java/Minecraft 工具根据已安装版本发现路径，多个候选时要求显式选择；构建目录可配置，避免旧 DLL 污染 release。
- 修复全工作区 Clippy 检查阻断项，包括公开裸指针接口的安全约定、内部接口可见性和无效比较，清理未使用常量与导入。

- 项目统一命名为 Kinakaze，源码发布在 `Kinakaze/Kinakaze`。
- 整理首页、上手文档、贡献说明、安全报告渠道与发布流程。
- 补齐 MIT / Apache-2.0 许可证正文及第三方声明导航。
- 增加 Windows CI 与标签发布工作流。

使用与验证见 [v0.1.0 运行说明](docs/runtime-release-v0.1.0.md)。完整 Linux ABI、容器隔离和干净 Windows 环境的独立部署仍未完成验收；历史应用结果见 [开发进度](docs/progress.md) 和 [验证记录](docs/validation.md)。工作区仍有风格与文档类警告，本版本不宣称零警告。
