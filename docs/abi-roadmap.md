# ABI 实现状态与完成路线

V2 延续项目现有自研 libc。普通模式必选，容器模式未来可选择客体 libc，但两种路径必须进入同一套 syscall/对象语义。目标是运行真实 BusyBox、curl、Minecraft Java 版客户端并继续扩展通用 Linux ABI；当前尚未达到完整兼容。

本文是当前实现与缺失操作的施工清单。强符号链接失败能定位下一处缺口，不能作为该操作的最终方案；导出数量、旧源码存在、返回成功和占位入口也不能证明 ABI 完成。实际通过的应用路径见 [验证记录](validation.md)。

## 已有运行结构

`engine/` 独立 workspace 的 guest engine、libc、VFS、TLS、分配器和其他 provider 实现以 rlib 形式只链接到 `kinakaze_runtime.dll`。外层每个 DLL 对应一个真实 ELF so，通过 PE 转发进入 runtime 的同一份实现和状态。公共 manager 负责逻辑 Process 身份、父子关系、fork/exec 状态事务和退出；需要公共状态的操作由 runtime 发起 RPC。

目前已有通用 Linux ELF 装载、严格 GNU 版本解析、数据对象/COPY 协调、TLS、同步异常与 syscall/FS 改写、原生 fork/exec 接续。实际 BusyBox、curl HTTP/TLS、进程 ABI、跨 worker 内存和 Java JIT/ProcessBuilder 探针已通过。不能继续把 V2 描述成仅三组函数或仅标量 fork 状态的工程。

同时，manager 的进程事务、worker 本地的 VFS/OFD/VMA、线程与 provider 状态还需要持续审查跨进程一致性。完整 `clone` 组合、namespace、异步信号和模块卸载等尚未完成应用级证明。

## 配对与符号完整性

构建从真实 PE 定义及 ELF 版本证据生成各模块标准 `.def`。运行时不使用模块清单；实际导出数量以打包检查为准。

- 函数和数据对象分开登记，数据大小/对齐取 guest ABI，不能用 Rust 包装对象的大小代替。
- 同一数据对象的别名必须指向同一底层存储；普通查找返回原始 DLL 数据地址，COPY 重定位必须通知 provider 切换到约定的 guest 存储位置，不能留下两个独立副本。
- 版本按 `(SONAME, symbol, version)` 精确匹配，并区分默认/非默认版本。观察到版本需求只证明这个程序需要它，仍要验证实现是否满足该版本 ABI。
- 打包前解析全部 PE 转发链，校验最终代码/数据和 BSS 边界，拒绝外部未登记目标、循环、空地址及类型错误。
- `ld-linux-x86-64.so.2` 有独立 DLL/ELF 配对；实际装载由 engine 执行。该依赖 facade 不等于完成 GNU loader 的全部私有 ABI。

新符号应同时登记调用约定、对象布局、版本来源、错误约定、副作用、引用与退出规则。对已迁入实现也采用同样标准补审查；不能把整批导出视作已经逐一验收。

## 各领域的实现与剩余方案

| 领域 / 当前来源 | 已接通或已有验证 | 剩余实现与完成判据 |
| --- | --- | --- |
| ELF、动态链接：`engine/crates/kinakaze-elf`、`kinakaze-link` | 真实 BusyBox/JRE 装载；严格版本、TLS、IFUNC、COPY 和动态查找已有实现 | 补全恶意边界、依赖图、符号抢占、RUNPATH、TLSDESC/各 relocation 组合和失败回滚；以独立 ELF 样例验证，不用一个 JVM 的成功代替所有重定位类型 |
| DSO 引用、init/fini 与展开：同上及 `engine/crates/guest-engine/src/host/loader` | link map/dlinfo、引用计数和部分析构已有实现；libc/libdl 入口已统一到进程装载器，部分 ELF 映射目前保留驻留 | 完成依赖引用图、NODELETE/RTLD_NOLOAD、TLS 析构与最后引用释放；统一 `.eh_frame`/CFI/LSDA 生命周期、guest/host 边界展开及 signal frame，验证 C++ 异常、栈回溯、卸载再装载和并发 dlsym |
| 字符串、数值、stdio：`engine/providers/libc` | BusyBox 文件/文本路径、Java UTF-8 路径通过；现有格式化、扫描、宽字符和 rand48 实现继续补齐 | 审查重叠/边界/溢出、GNU 扩展、locale 状态、宽字符流、锁和部分 I/O；curl 暴露的每个缺失入口补真实语义及错误分支 |
| libm、浮点环境：`engine/providers/libm` | 数学与 fenv 专项测试覆盖 x87/MXCSR，包含 80-bit long double 相关入口 | 按 SysV long double 传参与返回布局完善全部函数族，覆盖 NaN、无穷、次正规数、舍入、异常标志和 errno；历史 libc 数学符号须正确转发同一实现 |
| errno、TCB、TLS：`engine/crates/kinakaze-tls` | ELF TLS 和线程隔离专项测试、真实 JVM 多线程路径通过 | 补动态 TLS 扩展、DTV 代际、所有 TLS 重定位、dlclose/线程析构次序及多线程 fork 后锁恢复；跨 DLL 必须使用同一 Task 的 errno/TCB |
| pthread、futex、取消：`engine/providers/libpthread`、libc 线程接口 | 线程/退出/TLS 专项测试与 Java 异步任务通过；生命周期修复持续进行 | 完成 robust、PI、pshared、取消点、once、atfork、TLS key 析构与异常取消；跨 worker 等待使用可共享键和内核等待机制，不能依赖进程内 WaitOnAddress |
| Process、fork、exec、wait：`crates/manager`、runtime、`engine/crates/kinakaze-runtime` | 真实 fork 父子 PID、exec 同 PID、退出码和 wait、spawn 的管道/错误路径通过 | 扩大多线程和失败注入验证；exec 必须在旧映像停止执行后激活候选映像，包括重定位中的 IFUNC；补 clone 标志、vfork、zombie/reaper、进程组/session 与 namespace 生命周期 |
| fork 后 provider/RPC 恢复：runtime、shim SDK | 公共 prepare/adopt/ready/commit/abort 与 Copy/Share/Reset 已有验证 | 原生 child 必须选择自己已认领的 Session，不能使用复制来的父 HANDLE、序号或锁；重新初始化新加载 DLL 的 SDK 状态，再开放 guest 执行；覆盖 child 调用/close、父连接存活和父锁已锁定的情况 |
| VMA、映射、分配器：`engine/crates/kinakaze-alloc`、runtime 及 libc mmap | JVM 堆/JIT、原生 fork 和既有 VMA/COW 后端已接通 | 补文件尾页、部分解除映射、mprotect 拆分、mremap、MAP_SHARED/private、分配失败和内存账本；CLONE_VM 跨 worker 的 AddressSpace 身份、映射可见性和地址一致性须单独验证 |
| FDTable/OFD、文件与目录：`engine/crates/kinakaze-vfs` | BusyBox 文件/重定向、进程管道、Java 文件操作通过；继承与对象测试已有覆盖 | 审查 dup/fork 共享 offset、FD flags 独立、CLOEXEC、close/dup2 竞态、append、unlink 后引用、路径权限和跨 worker 缓存失效；manager 身份与本地 HANDLE 不能各自建立冲突状态 |
| socket、poll/epoll：VFS 与 libc 网络接口 | 已迁入 socket、readiness、epoll 后端及专项测试，curl 依赖链继续补齐 | 运行 curl HTTP/TLS 完整场景，并补非阻塞 connect、半关闭、EOF、edge/oneshot、FD 复用、取消与错误队列；以实际读写结果和 readiness 状态机为准 |
| DNS、NSS、locale：libc、`engine/providers/libresolv` | DNS 报文/名称压缩/结构布局测试通过；curl 暴露 resolver 与字符转换缺口 | 完成 resolver API 族、错误与重入接口、用户/组后端及 multibyte 状态；通过真实依赖路径验证，不能仅有能解析成功的一条域名 |
| signal、时间、timer：VFS signal、libc、`engine/providers/librt` | 同步异常路径支持 JVM；timed wait、timer 生命周期/回调及 SI_TIMER 数据测试已有结果 | 完成异步 Task 交付、pending/实时信号排队、mask、altstack、完整 ucontext/sigreturn、EINTR/SA_RESTART；timer 删除、超时和交付竞争需使用同一状态机 |
| POSIX AIO 与 io_uring：VFS I/O、`engine/providers/librt` | VFS/io_uring 已有实现与测试；POSIX AIO 仍未形成完整可用接口族 | 复用统一请求/完成队列，保持 aiocb 生命周期、部分字节数、aio_error/aio_return 单次回收、cancel、suspend、lio_listio 和 SIGEV 通知；对 ring flags、注册资源、CQ 溢出逐项验证，不能简单映射为宿主同名接口 |
| pipe、共享内存、IPC：VFS 与 libc/librt | 管道与部分共享对象后端存在，基础跨进程传输通过 | 用公共 namespace 管理命名对象、权限、引用和等待者；补 POSIX/System V sem/msg/shm、unlink 与最后引用分离、IPC_RMID、信号中断和 fork/exec 继承 |
| procfs、身份、跨进程内存：VFS、manager | 进程身份和跨进程内存授权接口已接入；JVM 运行使用部分 procfs 信息 | 补一致的 UID/GID、dumpable、PR_SET_PTRACER、process_vm_readv/writev、maps/FD/task 视图；检查权限、部分复制、目标退出和地址变化竞争 |
| 图形/窗口/输入：`engine/providers/libGL`、libX11/libxcb/libdisplay/libvulkan | provider 迁移及模块测试通过，尚无完整 Minecraft 客户端通过记录 | 从实际 Linux LWJGL/GLFW/JNA 调用推进窗口/context、GL/GLX/EGL/Vulkan 分派、显示模式、DPI、键鼠/剪贴板/窗口事件；验证线程归属、resize、窗口关闭与资源释放 |
| 音频：`engine/providers/libasound`、libpulse/libpipewire | playback 后端和真实静音 PCM 端点测试；线程锁/回调专项测试通过 | 补真实 capture 和 ALSA mmap begin/commit/ring 指针/可用帧/恢复语义；建立专用事件线程和回调生命周期，补 PipeWire 运行后的异步 core_sync；通过真实输出与可选择的录音验收，不用静音提交代表功能完整 |
| 容器、namespace、资源控制：整体设计与 manager | 普通 worker 模式已运行，Job 负责会话回收 | 实现 PID/mount/user/network 等 namespace 对象与边界、init/reaper、配额和隔离；容器 libc 可选但不能绕过 syscall 权限/对象服务。当前普通 guest root 不是完整容器安全边界 |
| 其他高级 ABI：逐 syscall 清单 | 各后端覆盖不均，需要逐项核对 | 对 ptrace、seccomp、capability、文件系统扩展和其余 ioctl/syscall 给出参数布局、状态转换、宿主后端及失败恢复；不把“返回 ENOSYS”登记为完成方案 |

上表说明全部实施领域，并不意味着每个领域中未列出的操作已经完成。逐 syscall 的目标方案继续参照 [完整设计](architecture-v2.md) 和 [覆盖清单](architecture-v2-syscalls.csv)；该清单是设计输入，不是当前 V2 的测试通过表。

## 应用验收的推进顺序

1. 保持 BusyBox、进程 ABI 与 Java 基础探针回归，修复任何 fork/exec、锁、TLS、文件映射和退出生命周期倒退。
2. 完成 curl 严格依赖链的所有真实 ABI，运行二进制 HTTP GET/POST、重定向、错误和 TLS 证书/主机名校验；继续补 DNS、代理和协议矩阵。
3. 在 Java 基础之上完成 ProcessBuilder、动态 native DSO 与 JNI 调用；从 Linux LWJGL/GLFW/JNA 的实际需求完善窗口、输入、图形和设备发现。
4. 使用独立 demo 目录启动 Minecraft Java 客户端，验证主界面/场景、键鼠、resize、音频、资源加载和退出，保留可观察证据；JVM 启动不代替此项。
5. 继续逐 ABI 完善并扩大参数/并发/错误场景，使未被三个应用调用的操作也有完整方案和实现。

这些阶段允许独立模块并行施工。任何应用通过后仍需保留明确的 ABI 缺口清单，直到对应结构、调用约定、错误、副作用、共享状态与生命周期都已有实现和验收。
