# 持续目标

2026-09-18 更新。保留当前长期 goal，以下要求合并此前功能目标，按独立可验证批次持续推进。

## 架构与交付

- 最终发布目录只含 `init.exe`、`worker.exe`、`rootfs/`。自研 DLL 原字节按 SO 名发布到 `rootfs/lib/`，第三方 ELF 依赖使用标准 lib 路径；构建链接输入、报告、SDK 与旧缓存不进入发布顶层。
- `ld.so` 负责混合装载、重定位、符号、TLS 与装载状态；VEH、指令转换、AOT/JIT 路径归 engine，通过 runtime 的显式接口协调。
- 不维护模块清单，包含全局清单、模块私有 TOML 和嵌入式模块描述；直接使用文件名、原生导出表与固定 C ABI。
- 不改写 DLL 导入表，不重定向整个标准库堆。模块私有内存自行管理；对外内存、内核句柄和生命周期走明确 runtime API 或 init RPC，并以模块序列化/恢复接口重建状态。

- 移除转发用的中转 DLL 和重复 ELF 跳板。自研实现可直接以 PE 格式、Linux SONAME 的 `.so` 文件交付；第三方 ELF `.so` 保持原格式。
- 建立明确的 ELF / PE 混合装载接口，维护已独立交付的 `ld.so`；移除旧装载空壳，不能在新路径可运行前删除必要功能。
- 缩小 runtime。模块只负责自身功能，通过明确、版本化接口共享进程、TLS、文件描述符等唯一状态，不能通过重复静态链接制造多份状态。
- fork / exec 明确区分模块重装、状态复制、共享、重建与释放；恢复过程可验证，失败明确返回，不能静默换实现。
- 清理无用环境变量、隐式默认路径、硬编码、兼容空壳和冗余代码；协议常量及标准规定的默认值保留并说明来源。
- 优化目录与构建职责；参考 `E:/Naka/crysoacu` 的真实实现和测试，不整块搬入旧架构。

## 功能与性能

- 实际运行 sshd/ssh、Java、Python、Node、Clang/GCC、FFmpeg、curl/wget、压缩解压、Bash、Debian 包管理、数据库和 agent 常用程序。
- 成组补齐 X11、XInput、RandR、ALSA/Pulse 接口及其内存与 fork 生命周期；持续扩展 nginx、构建工具、Git、文本处理、文件同步和进程工具的真实行为验收。扩展查询、版本启动和完整应用可用分别记录。
- 完整 Docker 能力纳入目标：先用真实 Docker / containerd / runc 行为验证进程、namespace、cgroup、mount、overlay、网络、Unix socket、生命周期和容器资源清理，再逐项收敛缺口。
- 实现 CUDA 桥接并逐步扩大已验证范围；不能将单个 CUDA 探针通过等同完整 CUDA 生态支持。
- init 提供简单 WebUI，展示真实基础和进程信息；不使用就不采集，不运行闲置轮询和后台诊断。
- 优先降低内存占用、拷贝、Unix socket 开销；汇编、JIT、AOT 优化必须保持 ABI 正确，并用真实测量判断收益。

## 验收方式

每批记录实际代码变更、运行结果、未覆盖边界与下一项。启动成功、功能通过和完整兼容分别描述。不使用假成功、静默 fallback 或扩大支持范围的说法，不因单点卡住停止其他进展。

当前已验证结果见 [progress.md](progress.md)；printf/随机数/网络接口验收见 `artifacts/printf-random-net-acceptance.json`；FFmpeg 与原生播放计时验收见 `artifacts/media-runtime-acceptance.json`；文件遍历、消息目录、iconv、线程接口、GPG 及代码审核验收见 `artifacts/review-runtime-acceptance.json`。最新 GNU 工具、QtCore 与桌面接口的分阶段验收和二进制哈希见 `artifacts/desktop-tools-acceptance.json`，独立产物位于 `artifacts/desktop-tools-dist/`；此前 GTK/GNOME 与 D-Bus 的失败保留为阶段记录；最新 Unix 凭据、D-Bus/dconf 和 ICCCM 行为见 `artifacts/unix-desktop-verified-matrix.json`，最新 GTK/GNOME 原生关闭与 AT-SPI 服务行为见 `artifacts/gtk-services-windows.json`、`artifacts/gtk-services-matrix.json`。目标未全部达到，不能标记完成。

## 桌面与工具覆盖目标（2026-09-18）

- 接近全面 Linux 工具覆盖作为长期方向；先固定软件样本和版本，分别统计依赖闭合、启动、实际行为、窗口交互和退出回收。没有 Linux 软件全集分母时，不宣称 99% 兼容。
- 本轮扩大真实 Debian GNU 工具、Qt5 示例、GTK3、GNOME Dictionary 和 D-Bus 依赖。依赖准备需同时保留库所属包的已审核插件、schemas、资源及显式包依赖；不要只复制 DT_NEEDED 指向的单个 SO。
- CLI、QtCore 序列化与原生 X11 绘图已完成阶段验收。后续补齐 Xcursor 图像、XSync 计数器与屏幕查询后，GTK/GNOME Dictionary 启动通过，GTK 窗口创建、事件循环和销毁通过；GNOME Dictionary 的建窗与原生关闭退出已通过，AT-SPI 总线及注册服务已验证。Qt XCB 平台还有独立协议缺口，完整 GNOME Shell/会话未验收。
- Unix socket 凭据与 SO_PEERCRED、D-Bus/dconf、GNOME 原生关闭与 AT-SPI 基础服务已通过。FD 存储已改为按需分层页表，最高编号 1,048,575、2048 个实际 FD 及 fork/exec 已验证，D-Bus 自身软限 65536 已验证。后续继续按需跨进程 FD 快照、GTK 更新事件、Qt XCB 与更完整的桌面交互。XI 抓取、完整 XKB/SYNC/Shape 扩展及动画光标仍未完成，不能计为功能完成；不通过忽略凭据或伪造扩展成功来提高通过率。

## 当前推进顺序（原生模块迁移）

本批已修复 Git `clone --no-local` 的原生 pthread 栈跨 guard 崩溃；最终产物 2/2 轮增强传输、8/8 次 clone 通过，记录见 `artifacts/git-stack-acceptance.json`。保留线程保留区与底部保护页，在进入 Linux ELF 前提交可用栈并同步 TEB；验证真实 pack/delta、对象完整性、多线程大栈帧和线程内 fork。不能仅用 VEH 补页承诺恢复，因为 RSP 落入未提交页时 Windows 可能无法进入用户态异常分发。

0. 已删除清单，移除 DLL/堆改写和运行时 ELF facade。原生导出工具收敛到 tools/native-exports，删除源码扫描 TSV、无消费者的构建环境变量和重复导出统计文件。TLS/ABI 切换内存显式使用 runtime 分配；装载器 fork 元数据通过二进制状态重建，loader-entry 与 native-math 已通过两代 fork。继续扩大实际程序验证，清理 Rust ABI 跨库依赖与其余私有状态恢复；不能将这两个探针推广为完整应用兼容。

1. services 宿主/常量回退已移除，查询/枚举分离并补齐 fork 游标及 `_r` ABI；网卡 ioctl 已接入真实 namespace 状态。真实 Docker 生命周期本批复测通过；容器内 IPv4 HTTP 通过，跨容器 IP 连接仍返回 EIO，未达到 DNS 验证。下一项以独立探针推进跨 namespace 路由、内置 DNS、IPv6 双栈、shim/runc 与 cgroup 清理，检查 daemon 存活时的资源回收。双栈的 IPv4→IPv6 通配监听已被独立 ELF 复现为 ECONNREFUSED；不能以 IPv4 测试替代双栈验收。route/ARP 固定数据已替换为按需真实视图，loopback 路由已修复；最小桥接测试的直接及跨原生进程 TCP 通过，仍需缩小与 Docker 创建/恢复流程的差异。继续替换 `/proc/net/tcp`、tcp6、udp、udp6 的旧固定数据。
2. 已将实际 ELF 链接器、dl* 与其 fork 状态移至 `libs/ld-linux-x86-64/src`，以 `ld-linux-x86-64.so.2` 原样交付；删除旧 engine 装载项目。libc/libdl 的标准 ABI 别名直接到达该实现；libdl PE forwarder 仍在，内部 Rust dylib ABI 尚未全部收敛为稳定 C ABI。loader-entry 验证 PE 与真实 ELF DSO 的构造器、TLS 和两代 fork。
3. Node 的 VM、工作线程、文件、环境、子进程与 Unix socket/TCP 行为已通过；SSH 已通过真实密钥登录、远程管道和退出码，继续验收 PTY、SFTP、PAM 与隔离。FFmpeg 的锁定依赖、Pulse 主循环及一批 X11 ABI 已补入，已新增 XResetScreenSaver、文本属性/WM hints 和字体尺寸入口，XChangePointerControl / XGetPointerControl 已接到原生设置并通过参数、错误、默认值和 fork 探针；SDL2 的指针、重挂接和 colormap 链接缺口已补齐。strtod_l / strtof_l 直接别名和 swab 已补入，ALSA 已补输出、格式掩码、PCM 信息/状态/设置/事件轮询、WinMM mixer、Raw MIDI 短消息与 MIDI 事件编码；printf 注册、可重入随机数和网络数据库的 9 个指定接口已经实现并通过实际探针，配套状态切换和回调内 fork 已验证。FFmpeg 已在独立构建中通过真实 FLAC 文件/管道无损往返、音频重采样和 FFV1 视频逐像素还原；Pulse 播放计时接入原生设备位置，暂停/续播/flush 和计时回调引用释放通过，实时录音、完整 server 语义及硬件编码继续验收。真实锁页范围操作与 fork 探针已通过；MCL_FUTURE/MCL_ONFAULT 明确未支持。文件遍历、GNU 消息目录/错误状态、真实 Unicode/Latin-1 转换以及线程补充接口已通过新增行为探针；GPG 验签和对称加解密已通过。2026-09-18 在逐文件哈希相同的 744 个稳定 ELF 范围内，版本化导入审计剩 8 项缺失（libc 7、ALSA 1）：pkey_* 五项、ptrace、sigqueue 与 snd_lib_error_set_handler；新增 Firefox 的更广范围另行验收，另有 13 项已观测 X11 缺失引用。继续以真实行为补齐剩余能力，不以错误返回入口或导出数量视为完整实现。GCC/Clang 的实际编译、线程/TLS/动态库/fork 探针以及 Redis 的 Unix socket、RDB/AOF 行为已通过；PostgreSQL 的 C/C.UTF-8 locale 与版本启动已通过，非 root exec 的 namespace 继承恢复已修复；继续处理 initdb 后续 VACUUM、locale 命令缺失接口和 SQL/重启恢复；不将版本启动当作数据库行为验收。stdio 私有状态通过模块快照恢复，spawn 的对外操作记录与路径使用客体内存；Java 默认 ProcessBuilder 连续 8 次行为和独立 posix_spawn 探针已在当前发布物通过，继续扩大覆盖。
4. Pulse 普通/线程主循环、XIM 上下文、按键状态和文本转换已落地，继续收敛流回调的线程归属，替换 Pulse 录音的旧占位路径，并实现活动音频状态的显式恢复。X11 属性字节、预定义/动态原子、WM hints 与客户端字体尺寸已经实现；已推进指针控制、窗口层级、坐标、堆叠、TrueColor 和显示宏对应 ABI；事件 next/peek 共用按需唤醒。当前 rootfs 真实 ELF 的 32 项缺失 X11 引用已收敛到 13 项，详见 artifacts/x11-interface-audit.json；优先成组实现字体、XKB、背景 pixmap、活动抓取和异步读取，继续补齐属性通知、XCB 共用原子状态、事件谓词与组合输入。完整 libX11 仍有更多公开/私有入口未实现，不能把当前程序链接通过当作整组接口完成。
5. 开发工具扩大到 binutils 与 gcov。binutils 静态库、符号/调试处理和处理后的程序执行已通过；gcov 正常退出已自动生成并解析 .gcda。ld.so 通过 RDX 交付 rtld_fini，自己的待执行析构队列进入 fork 快照；libc 单独管理和恢复 atexit/C++ 回调。main/DSO 顺序、重复 finalize、重新登记、递归 exit、普通 fork 以及析构内部 fork 均有实际验收。Git 传输栈崩溃已修复并在锁页批次再次通过 4/4 clone；继续处理 OpenSSL 依赖组合造成的 nginx TLS 阻塞。
6. 按需采集保持默认关闭；性能结论使用独立测量，文件尺寸下降不视为内存或速度提升。
