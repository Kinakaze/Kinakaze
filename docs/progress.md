# V2 持续推进

## 2026-09-19 WebP 解码与 GNOME 启动修复

- 将 Debian webp-pixbuf-loader 及真实 libwebp 依赖锁入 GNOME Shell / backgrounds 闭包，安装到客体并更新 GdkPixbuf 缓存。验证透明无损、有损、7 字节分段读取、截断错误和原始 4096×4096 壁纸；未转码替换图片。
- 修复 fork 后递归 mutex 的原生 SRW 等待队列指针：重建锁字，保留 owner / recursion。加载器冻结前阻止新的非持有者进入 typed mutex，限时等待已有短临界区结束；长期持有的锁仍按 POSIX 保留归属，未强制解锁。
- 修复线程退出与 fork 冻结的竞争：SuspendThread 失败时仅跳过已确认结束的线程，保留枚举句柄到游标推进；其他错误仍报告。
- 修复 XGetGeometry / XGetWindowAttributes 请求序号及 XNextRequest 的下一请求语义；原生请求先执行 ELF 扩展缓冲区，避免 XCompositeNameWindowPixmap 后立即查询得到 BadDrawable。真实 Composite / GDK 回归通过。
- 最终四项客体回归通过（WebP、GDK trap、递归锁 fork、Composite 位图查询/像素/生命周期），pthread 40 项、fork 相关 30 项原生测试通过。报告见 artifacts/gnome-ordered-regressions.json 及 gnome-pthread-unit.log、gnome-kernel-unit.log。
- 17:18 的实际 Shell 会话已完成启动，顶栏、壁纸、Activities、Settings 和 Terminal 均有现场绘制证据。保留当前会话供使用，并将 39 个发布文件逐字节校验后同步到 artifacts/gnome-full-dist。现场状态见 artifacts/gnome-startup-fix-result.json；完整系统服务、所有设置项和拖动交互未据此宣称通过。


## 2026-09-18 GNOME Shell 实际启动、功能补齐与桌面坐标修复

- GNOME Shell 43.9 / Mutter 43.8 已实际绘制桌面，Activities、日历和设置窗口可出现；真实 system/session D-Bus 与 GNOME 数据、GI、IBus、Evolution 等依赖已准备，新增 tools/run-gnome.py / docs/gnome.md 提供复现。以下旧条目保留当时的失败状态，本条记录后续结果。
- 实现 XKB 基本服务端状态、Composite/Damage 保留表面、GLX 纹理读回、XFixes 跟踪/指针屏障、XI2 事件、SYNC alarm/fence，以及真正作用于像素的 GC 区域/位图裁剪。修复 JIT 地址空间重用、Pulse GLib 异步操作、procfs 跨进程身份、未知 sysfs 路径及部分 libc/math ABI。dlopen 失败恢复对象图，但保留单调 TLS ID，未恢复危险的编号复用。
- 用户反馈的鼠标偏移实测为 overlay 的 Windows 装饰导致客户区原点 (8,31)、高度 1421。桌面 overlay 改为无边框并恢复精确屏幕矩形，同时避免全局 TOPMOST；最终实机原点 (0,0)，客户区 2560×1440。真实 libXcomposite 坐标/指针探针通过。
- 分阶段验证：相关 Rust 模块 662 通过、0 失败、3 忽略；最终桌面矩阵 9/9；GI 75 个 typelib / 53 个实际库装载通过；16 个静态闭包无缺失强符号或版本；Nginx 2/2 通过。最后的 overlay 修改以最终行为矩阵和实际窗口测量验证，没有把此前单元测试算作该修改后的重跑。
- 完整 GNOME 尚有 PipeWire 采集、GDM/logind/SessionManager、NetworkManager/bolt、布局上传、Pulse 录音等缺口和部分启动告警，不宣称所有桌面功能完成。实现、证据和复现详见 artifacts/gnome-implementation-result.md。

## 2026-09-18 useradd/groupadd 八个 libc 接口

- 已实现并发布 `__fgets_chk`、`fgetspent`、`sgetspent`、`putspent`、`putpwent`、`putgrent`、`lckpwdf`、`ulckpwdf`。账户记录解析/输出与密码锁分别放在 `libc/userdb/account_files.rs`、`password_lock.rs`，行读取复用 stdio 的单次 FILE 锁；返回数据使用 guest 内存。
- 对照 glibc 行为处理旧格式、NIS 记录、空字段、非法字段和长行；密码锁使用真实 POSIX 记录锁、15 秒 SIGALRM 超时，并恢复调用者信号状态。fork 保存锁描述符状态，父进程记录锁仍归父进程。
- 审核并修复 stdio 对只读流缓冲写入误报成功的问题；fopen/fdopen、freopen、回调流和 fork 恢复均保留读写模式。fortify 溢出通过 guest SIGABRT 终止，使父进程正确取得信号退出状态。
- 当前发布包的三组真实行为测试全部通过：账户文件/fortify/fork、密码锁竞争/唤醒/超时、Debian useradd/groupadd/userdel/groupdel 创建/重复拒绝/主组保护/删除；测试账户删除后 passwd/group 内容恢复原样。见 `artifacts/account-api-verified.json`。另有 11 项 stdio 和 1 项 freopen 原生测试通过。额外的 cookie-streams、desktop-core-streams guest 探针因该 rootfs 缺 gcc/coreutils 未运行，不计为通过。
- 已用真实 useradd 创建 messagebus 账户，消除此前的缺账户问题；GNOME 最近一次实机启动仍在系统总线连接处报 `Could not connect: No such file or directory`，完整桌面尚未成功。本批没有改动用户撤销的 loader/TLS 路径。

## 2026-09-18 GNOME 依赖补齐与撤销后的复验

- 以用户手动撤销后的工作区为准，未恢复 loader/TLS 的加载失败清理改动。重新构建并打包当前源码，29 模块 / 5381 导出检查通过；依赖工具 19 项测试通过。
- GNOME 使用一致的 Debian rootfs，补齐会话定义、Mutter/GJS/GI 资源、AT-SPI、字体、图标与桌面缓存；本轮继续补入 Json typelib、libgweather 的 Locations.bin、SVG 图片插件和系统 D-Bus 配置，均记录包及文件校验值。资源由真实 Debian 包提供。
- Shell 已越过 XIGrabTouchBegin 和 Json/GWeather 缺失，执行到 panel/quick-settings 的系统菜单初始化；当前 session-bus-only 启动因系统 D-Bus 不存在退出 1。真实 system dbus-daemon 又确认缺 messagebus 系统账户。没有将超时、空窗口或 RUNNING 日志算作桌面成功。
- 安装标准 passwd 包用于创建系统账户，补齐其 libsemanage 依赖所需的 lfind，并同时实现 lsearch；独立模块只操作调用者数组，不分配内存。首个匹配地址、比较顺序、未命中插入、元素计数与 errno 测试通过；与 FTS、GLX、XCB/EGL 共 4/4 通过，见 artifacts/gnome-after-undo-regression.json。
- useradd/groupadd 的下一组明确缺口为 __fgets_chk、fgetspent、sgetspent、putspent、putpwent、putgrent、lckpwdf、ulckpwdf；messagebus 尚未创建，完整 GNOME 尚未成功。当前证据与复现入口见 artifacts/gnome-startup-current.md / .json。

## 2026-09-18 GNOME Shell 与完整会话早期实机检查

- 实现 `glXQueryContext` 的上下文元数据查询、失效句柄与属性错误检查，并声明已实现的 `GLX_ARB_get_proc_address`。新增 `desktop-glx-context` 验证 libGL/libGLX 两条入口、三种上下文创建路径、绑定前后查询和销毁后失效；与 `desktop-xcb-egl` 回归一起 2/2 通过，见 `artifacts/gnome-glx-regression.json`。
- GNOME Shell 43.9 版本检查、独立 GL helper、完整 `gnome-session-check-accelerated` 均退出 0，实际识别 RTX 4060。用 guest 的 glib-compile-schemas 严格重建过期缓存，SessionManager schema 错误消除。工作区构建和 29 模块/5317 导出检查通过，未重跑或沿用全量测试作为本次验收。
- 纳入工作区最新 GLX/XRandR 修正后，最后一次 Shell 越过此前空调用与 XRandR 崩溃，现停在 `XIGrabTouchBegin` 的 BadRequest（128.54），退出 1。完整会话还缺有效 `gnome.session`、会话服务、AT-SPI、fontconfig 和图标资源；其 RUNNING 日志不是桌面启动成功。具体日志、复现命令、测试包与文件哈希见 `artifacts/gnome-startup-current.md` / `.json`；本次测试进程已清理。

目标创建于 2026-09-16，保持进行中。范围：模块化和标准化、清理冗余、常用 Linux 工具与 agent 工作流、进程与系统信息、init WebUI、CUDA 直通，以及内存、Unix socket、汇编/JIT/AOT 的性能改进。交付以真实程序的成功行为、故障与退出回收为依据。

用户补充约束：**不使用就不采集**。监控不得向正常运行路径添加后台周期采集。

## 2026-09-18 Qt 窗口尺寸与绘制区域实测

- 修复 Qt 日志中的 SendEvent BadImplementation：Xlib/XCB 非传播发送按目标窗口订阅掩码筛选接收者；零掩码仍直送所有者，非零掩码没有订阅者时成功且不投递。非法掩码/窗口继续报协议错误，事件负载与 send_event 标志保持正确。传播至祖先窗口仍未实现，本批不声明完整窗口管理协议。
- 首轮实测确认已有尺寸通知与绘制路径可用，因此没有重写 resize。新增 `tests/guest/desktop-resize.py`，在测试 Job 中仅操作自己的 Qt 时钟 HWND；读取真实客户区显示像素，不调用 PrintWindow 或手动 WM_PAINT。比较紫色时标外接范围与基于客户区短边的预期位置，检查四角背景。
- 八项像素验收全部通过：320×240、700×460、260×520、180×160、连续五次调整后的 480×320、最大化 2560×1369、恢复，以及最小化后恢复。表盘跟随尺寸变化重新居中和缩放，未发现检查范围内的旧边缘残留；退出 0，Qt stderr 无 XCB error。截图及测量见 `artifacts/qt-resize-after.json`。记录耗时含截图、像素分析和保存，不作为渲染性能基准。
- 扩展 XcbSelectionProbe，验证有/无订阅者、混合掩码、非法掩码/窗口、Xlib→XCB 消息负载，以及既有选择区与 fork 行为；探针通过，见 `artifacts/qt-resize-events.json`。整包 36 个原生镜像字节一致和空 PATH 启动验证通过。只对 Qt 时钟进行了本批像素验收，不推断所有 GUI 程序均支持所有缩放/显示场景。

## 2026-09-18 Qt 启动与关窗死锁修复

- Qt Analog Clock 的主线程在辅助功能初始化中等待 QDBusConnection 的阻塞调用；工作线程已经完成连接，但 QSemaphore::release 使用的 `FUTEX_WAKE_OP_PRIVATE` 原先返回 ENOSYS，等待者无法被唤醒。线程启动互斥锁与 XCB 关闭消息均正常，修复不禁用 D-Bus 或辅助功能。
- libc 实现 WAKE_OP 的 SET/ADD/OR/ANDN/XOR、12 位有符号操作数、移位及六种旧值比较；原子修改与两组唤醒共用等待队列锁，支持同地址且不重复唤醒。唤醒选择直接返回数量，取消每次唤醒的临时 Vec 分配；临时线程/轮询诊断已删除。
- 7 项 futex 回归通过，覆盖新增原子操作、条件双队列、同地址数量限制，以及既有超时、重排队、位掩码、信号打断和重启。`desktop-window.py` 新增真实 Qt Analog Clock；最终包的 GTK、GNOME Dictionary、Qt 三项建窗/原生关窗均退出 0，见 `artifacts/qt-close-final-windows.json`。
- 已重建 `artifacts/desktop-tools-dist`，顶层仅 init.exe、worker.exe、rootfs；36 个原生镜像与构建字节一致，空 PATH/独立工作目录启动通过，29 模块/5295 导出检查通过。本次针对性验收见 `artifacts/qt-close-acceptance.json`，未把此前全量测试算作本次重跑。本批当时存在 SendEvent BadImplementation 警告，后续已修复非零掩码接收者筛选并验收，见上方尺寸与绘制区域记录；完整桌面协议继续推进。

## 2026-09-18 XCB 绘图、输入、选择区与 Qt 窗口路径

- 补齐 Qt XCB 依赖链审计中最初缺失的 55 个入口；按查询/属性、绘图、输入、窗口、选择区、资源及生命周期分工实现。像素探针验证图像格式、位图、16 种栅格操作、平面掩码、裁剪、重叠拷贝及请求时快照，未把导出存在算作功能完成。
- Xlib/XCB 共享连接、原子和属性；阻塞取事件使用独立 eventfd 唤醒。补键盘控制、焦点和异步键盘抓取；同步抓取仍明确不支持。属性通知只在选中 PropertyChangeMask 时产生，支持改写、删除、读取后删除与 fork 恢复。
- 选择区握手、colormap、位图光标和窗口持有均有实际资源与错误路径；Xlib 创建的 ARGB 光标可交给 XCB 设置，不可变像素使用 Arc 共享，调用方释放后窗口继续持有。修复 fork 事件转移在连接冻结后再次取锁造成的死锁，在冻结前通过独立 prepare 阶段转移事件。
- libdisplay 保存 XCB 资源编号到可重建窗口的映射，EGL 据此获取实际 HWND；子进程重建、像素读回和销毁后编号失效均通过。纠正同一 visual 被同时列为 24/32 位的问题，32 位 pixmap 不再冒充另一个窗口 visual；映射/取消映射接入共享 Xlib 窗口状态和事件。
- 新增五组实际 XCB 行为探针：core、drawing、input、selection、egl；其中 selection 覆盖选择区、事件订阅、光标、colormap 及 fork，egl 比对父/子进程实际 GL 像素。最终结果见 `artifacts/qt-xcb-verified-matrix.json`、`qt-xcb-verified-workspace-tests.log` 和 `qt-xcb-verified-windows.json`。
- 本批最终验证：1777 个 Rust 测试通过、0 失败、24 忽略；12 项实际桌面行为和 GTK/GNOME Dictionary 两项原生关窗通过。29 模块 / 5295 导出检查、36 个原生镜像字节一致、空 PATH 启动和 smoke 资源归零通过；汇总 `artifacts/qt-xcb-acceptance.json`。
- Qt Analog Clock 已创建真实命名窗口，EGL 的无效 HWND 和共享光标 BadCursor 已消除；本批当时原生关闭未退出，后续已定位为缺失 FUTEX_WAKE_OP 并修复，详见上方 Qt 启动与关窗验收。GLX 插件仍缺 glXDestroyPbuffer；完整 XKB/XI、GNOME 会话、活动 GL 上下文 fork 与窗口管理协议继续推进。

## 2026-09-18 描述符容量、按需存储与桌面服务上限

- 用独立 `kinakaze-vfs/fd_slots` 分层页表替代预分配的 1024 项数组和独立预约集合，最高 FD 编号为 1,048,575。64 项一页，位图维护已用/预约状态及最低空位，空页回收；fork/exec、procfs 和句柄枚举跳过未分配页。实际表存储：三个标准 FD 为 3,120 字节，追加最高编号 FD 为 8,224 字节，旧表为 40,960 字节；此数字不含分配器开销，也不是整个进程 RSS 或吞吐测量。
- RLIMIT_NOFILE 默认软/硬限为 1024/4096，特权服务可按需提高；降低后的软限仍约束新 FD 分配，已有高编号 FD 保留。硬限提高检查与更新在同一进程表锁内完成；跨用户命名空间不能仅凭本地能力提高硬限。`sysconf` 和 `/proc/<pid>/limits` 返回实际 NOFILE 状态。
- `select/pselect` 拆至独立文件，保留标准 1024 位 `fd_set` 类型，同时按 nfds 处理调用者提供的位图。修复小位图被完整结构清零覆盖、同一 FD 同时读写只计一次、无 FD 等待立即返回、无效 FD 未报 EBADF 等问题。`close_range`、spawn closefrom 和 sync 改为遍历实际打开的 FD；UNSHARE 尚未实现，明确返回 EOPNOTSUPP。
- 真实 Python 验证最高编号 FD、2048 个同时打开的描述符及 fork 继承、共享文件偏移、select/poll/epoll/eventfd、SCM_RIGHTS/CLOEXEC、软限耗尽和空位复用、硬限权限、fork/exec 及 close_range。spawn 的实际 C 程序验证高编号 FD 被关闭；D-Bus 查询自身 PID 后读取 procfs，验证服务软限确实达到 65536，并保留 dconf 持久写入检查。
- GTK/GNOME Dictionary 原生关闭、AT-SPI 总线/注册服务、Unix FD/凭据传递和 Git 四次强制传输保持通过。分阶段报告为 `artifacts/fd-capacity-final-matrix.json`、`fd-capacity-regression.json`、`fd-capacity-windows.json`；存储测量见 `fd-capacity-storage.log`。
- 最终 Rust 回归 1766 通过、0 失败、24 忽略；实际程序 11 项行为、1 项 Java 启动通过。29 模块/5108 原生导出检查通过，36 个发布原生镜像与构建字节一致；顶层仍仅 init.exe、worker.exe、rootfs，空 PATH/无关工作目录启动通过，smoke 退出后 processes/objects/transactions 均为 0。完整证据及代码哈希见 `artifacts/fd-capacity-acceptance.json`。
- 本批提高的是 FD 编号空间和按需容量，未验证同时打开百万 FD。跨进程 proc-FD 共享快照仍有 8192 条总额，后续需拆为按进程按需存储；不同 PID 的 prlimit 身份权限还需完善。Qt XCB、完整 XKB/XI、GNOME Shell/会话等目标保持进行中。

## 2026-09-18 GTK/GNOME 事件唤醒与辅助服务

- 修复 GTK 的临时 XOpenDisplay/XCloseDisplay 注销主窗口通知的问题：共享原生连接按打开引用管理，最后一次关闭才注销通知、释放 eventfd 和清理扩展状态；分离到 libX11/connection。移除固定 fd=3 与函数指针缓存；描述符创建失败明确返回空连接。
- fork 通过模块快照恢复引用数及连接描述符编号，重建私有事件计数器，防止子进程消耗或触发父进程的通知。实际探针覆盖重复打开/关闭、属性保留、CLOEXEC、父子隔离与最终 FD 回收。
- 锁定并安装真实 Debian at-spi2-core 依赖，补齐其五个 X11 装载入口。XAllowEvents 验证模式并处理当前未冻结输入状态；XKB setters 按扩展未提供的客户端契约返回 False，未宣称支持 XKB 控制、同步抓取或回放。
- GTK 无定时器的 gtk_main 收到原生 WM_CLOSE 后退出；GNOME Dictionary 的建窗、自动激活 dconf/AT-SPI、关闭退出通过。新增 tests/guest/desktop-window.py 使用独立宿主 Job 确认窗口归属，避免向其他任务窗口发送消息。
- D-Bus/dconf 与 AT-SPI Bus.GetAddress、注册服务自动激活、桌面对象 introspection/GetRoleName/GetChildren 均通过。见 artifacts/gtk-services-windows.json、artifacts/gtk-services-matrix.json 和 artifacts/gtk-connection-matrix.json。
- 最终验收：8 项不同桌面行为通过；全 workspace 1763 通过、0 失败、24 忽略；29 模块 / 5108 导出一致。发布顶层三项、36 个原生文件逐字节匹配、空 PATH/无关工作目录运行与资源归零通过。汇总 artifacts/gtk-services-acceptance.json。
- 当前仍有 GTK 更新计数警告及 D-Bus 提升到 65536 FD 被拒绝；共享单显示模型、完整 XKB/XI 抓取、Qt XCB、GNOME Shell 会话和活动 GUI 全状态恢复仍需推进。这些通过项不等于完整 Linux/桌面兼容。

## 2026-09-18 Unix 凭据、D-Bus/dconf 与 ICCCM

- 实现 `SCM_CREDENTIALS`、共享 `SO_PASSCRED` 和真实 `SO_PEERCRED`：显式身份校验 real/effective/saved IDs 与命名空间能力，消息采用发送时真实 ID，连接采用 connect/listen 时 effective IDs；保存 PID 命名空间编号，使发送方退出/exec 后身份仍可恢复。未启用且未显式提供凭据的普通发送不附加身份记录，相邻相同身份的记录合并。
- 凭据先于 SCM_RIGHTS 输出，处理部分读取、MSG_PEEK、控制缓冲区截断、CLOEXEC、dup/fork/exec，以及非 root 凭据防伪；修正 getsockopt 短缓冲区返回长度。Windows 管道连接属性由服务端发布，客户端不写服务端属性。
- 实际 D-Bus 会话的 ListNames/GetId/introspection 通过；dconf 能被自动激活，并在隔离配置目录完成 gsettings 写入、再次读取及持久数据库创建。上一阶段的凭据不支持故障已修复。
- 补实 XSetWMProtocols、XSetClassHint、XSetNormalHints、XSetWMNormalHints、XGetWMNormalHints；新增 XGetWMProtocols、XGetClassHint、XSetWMSizeHints、XGetWMSizeHints、XGetNormalHints。查询返回使用客体内存并支持 XFree；尺寸查询只读取协议规定的字段，支持旧格式和未知尾部字段。原生导出为 29 模块 / 5103 项。
- 完整 Rust 回归 1763 通过、0 失败、24 忽略；7 项实际回归通过，最后的短缓冲区/尺寸属性补充由 5 项用例复验。见 `artifacts/unix-desktop-final-matrix.json`、`artifacts/unix-desktop-verified-matrix.json`。Git 强制传输仍通过四次 clone 与 fsck。
- GNOME Dictionary 可创建和绘制窗口、激活 dconf，但 WM_CLOSE 后仍未退出；线程栈与按需诊断确认 UI 线程仍运行、X11 eventfd 写入成功，后续需检查 GTK 事件/帧时钟调度。AT-SPI 服务仍缺失，D-Bus 请求 65536 fd 上限仍被现有 1024 描述符硬上限拒绝；不宣称完整 GNOME 或 99% 工具兼容。

## 2026-09-18 后续补充：GTK 窗口与 GNOME 装载

- Xcursor 改用真实 ARGB 图像、热点和原生光标，移除 1×1 占位及图像地址旁表；读取客体主题文件，图像使用客体内存，窗口独立复制原生光标，客户端释放后仍可使用。补 XSync 64 位数值/计数器及模块 fork 恢复、原生 monitor 快照和默认矩形查询；完整 SYNC/Shape 扩展与动画光标仍不宣称支持。
- 修复 GTK 的两个真实崩溃：GLX 厂商字符串被错误当作版本字符串返回；Xlib 的 `_XLockMutex_fn`/`_XUnlockMutex_fn` 被导出成函数而不是函数指针数据。现在使用正确数据 ABI 和原生递归锁。补 XPeekIfEvent，修复条件取事件丢弃未匹配事件，临时缓冲改为可释放的客体内存。
- 修复 X11 创建即显示的问题，窗口在 XMapWindow 前保持隐藏，映射状态进入现有窗口生命周期；补映射事件、避免重复通知，并给 Cairo 所需的 Screen.depths 提供实际 visual/depth 表，修复原有“数量为 1、指针为空”的崩溃。
- 补锁定的 gsettings-desktop-schemas（包总数 421）及 GdkPixbuf 的真实插件/查询程序。`tools/prepare-desktop.py --dist <发布目录>` 使用客体工具生成 schemas、图片解码、GIO 与 MIME 标准缓存，并准备登录用户的 XDG 目录和持久 machine-id；没有后台采集。
- GTK 窗口创建/显示、事件循环、销毁，以及光标图像、计数器 fork、事件筛选和屏幕结构的行为探针已通过；当前全 workspace **1763 通过、0 失败、24 忽略**。原生导出为 29 模块 / 5098 项，产物仍是顶层三项和 36 个原字节 SO。最终分阶段结果见 `artifacts/desktop-tools-acceptance.json`。
- GNOME Dictionary 已通过 `--help` 启动并创建真实窗口，原测试的 `--version` 不受该程序支持，已修正。GNOME 关闭探针仍失败，dconf 的 SCM_CREDENTIALS 发送仍返回不支持，另有 AT-SPI 服务缺失；GTK 窗口探针不等于完整控件渲染、输入或 GNOME 桌面会话验收。继续推进这些实际缺口，不宣称 99% 覆盖。

## 2026-09-18 第一阶段：GNU 工具、QtCore 与桌面接口

- 扩大锁定的 Debian 软件范围，新增 102 个包（318→420），包含 GNU 文件/文本/压缩工具、编辑器、GTK3、GNOME Dictionary、Qt5 示例与 D-Bus。依赖准备现在保留隐式库所属包的已审核资源、schemas、插件及显式依赖，修复只复制单个 SO 时资源缺失的问题；准备报告验证 11033 个文件和 3220 条依赖边。包管理的安装脚本未执行，不能将文件准备等同完整 Debian 安装行为。
- 修复三个真实 GNU 工具故障：FILE 的 EOF/error 标志同步到 Linux ABI 可见字段，解决 sort/base32/base64 到 EOF 后循环；aligned_alloc 接受 GNU 非整倍数大小，解决 split 分配失败；strtold/strtold_l 使用真正的 80 位解析和 x87 ST0 返回，解决 seq 的长双精度 ABI 错误。浮点解析使用 rustc_apfloat，依赖许可随标准 rootfs 文档交付；未宣称完整浮点异常标志支持。
- 新增 fgetpos/fsetpos 及直接 64 位别名、调度属性 getter、canonicalize_file_name、futimesat、sigset 和检查版宽字符/缓冲读取入口。现代 libc 定时器入口直接转发既有 librt 实现，共享计时器状态和生命周期。ABI 探针覆盖小对齐与非整倍数分配、长双精度精确值、stdio 标志/fork、文件位置、时间、信号及版本化定时器入口。
- X11 绘点支持真实像素、16 种 GC 运算、平面/裁剪掩码及相对坐标；背景 pixmap 采用独立所有权，支持 None/ParentRelative，原 pixmap 释放后仍可重绘。补 modifier 映射、WM 属性和文本转换，修复仅改变窗口宽度时损坏高度的问题。XI 查询、核心光标与 Device Enabled 属性已接真实状态；未实现的 XI 抓取和 XKB 扩展明确报错，不能把这些入口计为功能通过。实现拆分到 graphics/background、graphics/points、wm_properties、Xi/query 与 Xi/grabs。
- 新行为探针通过 GNU 文件/文本/流处理、哈希、gzip/bzip2/xz、tar/zip、gawk、diffutils、ripgrep、fd、tree、bc、Vim 实际编辑、GIO 文件操作和 QtCore JSON/CBOR 保存加载；X11 原生窗口的像素、属性、背景及部分尺寸更新通过。nano/less 仅验证启动，QtCore 结果不代表 Qt GUI/XCB 已可用。
- 主回归 **43 项中 40 通过、3 失败（37 个行为通过、3 个启动通过）**，包含 Git 4/4 clone、对象校验及既有线程/fork/X11 回归。此后补定时器导出并重建，针对性定时器/文本/QtCore **3/3** 与 XI **1/1** 通过；各阶段二进制哈希分别保留，未把早先整组结果标成最新映像的全量重跑。全 workspace **1762 通过、0 失败、24 忽略**，文档测试通过；该全量测试位于最后的定时器导出映射之前，Rust 实现随后未变化。导出工具 20 项、依赖工具 18 项通过。
- 最终产物 **artifacts/desktop-tools-dist**：29 模块 / 5061 导出，顶层仅 init.exe、worker.exe、rootfs；36 个原生 SO 与构建 DLL 原字节一致，空 PATH/无关 cwd 启动通过，原生 smoke 的进程、对象、事务全部归零。更大样本 **886 个 ELF / 17832 项版本化导入要求** 的缺失从 29 降至 7（pkey_* 五项、ptrace、sigqueue）；不与上一批 744 个 ELF 的分母混用，也不把版本化导入闭合等同 GUI 行为。分阶段记录与哈希见 **artifacts/desktop-tools-acceptance.json**。
- 三个失败保留为下一批入口：GTK3 和 GNOME Dictionary 均在 XcursorShapeLoadCursor 处装载失败，后续还有 Xcursor/XSync/RandR/Shape 缺口；D-Bus 会话连接缺 SCM_CREDENTIALS 传输，另有 fd 上限提升被拒绝。完整 GNOME Shell/桌面会话、Qt GUI/XCB、XI 抓取、活动 GUI 的 fork 恢复尚未验收。没有 Linux 软件全集分母或性能基准，不宣称 99% 覆盖及量化性能提升，长期 goal 保持 active。

## 2026-09-18 前批：文件遍历、消息目录、字符转换及审核修复

- 新增 ftw/nftw 及 64 位直接别名；迭代遍历 VFS，支持物理/逻辑链接、去环、深度顺序、CHDIR 恢复与 GNU action 返回值。路径、目录快照及遍历帧使用客体内存，回调中 fork 后父子均可继续；无原生递归，每次目录读取后即关闭遍历 FD。128 层目录、链接、删除竞态、停止/跳过和回调内 fork 已通过。
- 新增 GNU 消息目录 catopen/catgets/catclose 与真实 gencat 所需 __open_catalog、__mempcpy；校验文件布局、表边界及字符串终止，返回对象全部使用客体内存。实际 libc-dev-bin 的 gencat 已完成新建和增量更新，路径/locale 替换、大小端头、损坏输入与 fork 均通过。GNU error 的计数、前缀及去重状态独立放入 stdio/gnu_error.rs，并通过模块生命周期恢复 COPY/GOT 指向及状态。
- 用真实 iconv 替换原有字节复制占位，支持 ASCII、Latin-1、UTF-8、显式大小端 UTF-16/32 及 Linux WCHAR_T；检查非法/不完整字符、输出空间与输入输出位置，转换器可跨 fork。未支持的编码及 TRANSLIT 明确返回 EINVAL。补 libc 到现有 pthread 的 clockwait/affinity 入口及 thrd_exit，共用线程退出和 TLS 析构；绝对超时向上取整并在原生超时后重查截止时间。
- 审核新增线程/信号/GPG 改动，同时修复三个已复现问题：构建参数的反斜杠转义使 PowerShell 7 的 Cargo 配置解析失败；libnpth 两个文件占用相同查找名称导致整个依赖准备失败；消息目录错误地使用 64 位乘法，导致真实 gencat 大编号消息查找失败。构建参数已在 PowerShell 5/7 下验证；锁文件保留两个真实文件、明确各自名称并去掉整文件 CRLF 噪音；目录哈希按 [GNU gencat](https://raw.githubusercontent.com/bminor/glibc/master/catgets/gencat.c) 的 32 位乘法和 size_t 转换处理，正/负回绕消息均通过。原始失败报告保留于 artifacts/review-*。
- pthread_tryjoin_np 测试改为 barrier 与原生线程完成通知，去除固定 sleep 的竞态。新增 timed rwlock、tryjoin 和信号名称接口通过本次全工作区测试。GPG 的版本探针修正为 startup；行为探针使用临时 home，直接检查验签退出码，禁用对称口令缓存，并按 [GNU agent command 模式](https://www.gnupg.org/documentation/manuals/gnupg/Agent-Commands.html) 管理测试 agent。验签、篡改拒绝、AES256 加解密逐字节一致与错误口令拒绝均通过；--package gpg 自动包含 gpg-agent，55 个文件与 122 条 ELF 依赖边均验证。
- 最终独立产物 **artifacts/review-runtime-dist**：**13/13 探针通过（12 行为、1 启动）**，包括 Git **4/4 clone**、对象校验、多线程大栈、printf/random/quadmath 和新增功能。全 workspace **1760 通过、0 失败、24 忽略**，文档测试通过；导出工具 20 项、依赖工具 17 项通过，29 模块 / 5017 导出检查通过。顶层仅 init.exe、worker.exe、rootfs；36 个原生 SO 与构建 DLL 原字节一致，空 PATH/无关 cwd 启动通过，smoke 退出后进程、对象和事务均为零。汇总与二进制哈希见 artifacts/review-runtime-acceptance.json。
- 相同且逐文件 SHA-256 未变化的 **744 个 ELF / 9929 项导入要求**，缺失从 14 降到 **8**（libc 的 pkey_* 五项、ptrace、sigqueue，以及 ALSA 错误处理回调）。早先 walk-catalog-dist 的 11 项通过记录保留为历史基线；审核发现的大编号错误已在新产物修复，历史验收明确标记 superseded。
- 尚未覆盖：FTW 实际跨挂载/权限拒绝、完整字符集和有状态 iconv、GnuPG 密钥生成/智能卡/密钥服务器。timed rwlock 仍使用有界 sleep/backoff，完整处理器组 affinity 与 cancellation 仍未完成；不宣称已达到极限性能。Docker 跨 namespace 路由、Pulse 真实录音等原目标继续保留，整体目标未完成。

## 2026-09-17 前批：FFmpeg 文件转码与 Pulse 原生播放计时

- 重新构建并生成当前 XCB/Pulse 导出后，FFmpeg 已通过装载与音频生成处理。新增 `FfmpegRuntimeProbe.py`：真实 WAV→FLAC→PCM 的样本逐字节一致，FLAC 管道输入/输出一致；44.1 kHz 双声道重采样为 48 kHz 单声道，确认 4800 帧与非零数据；6 帧 RGB 视频编码为 FFV1/Matroska 后解码，逐像素与原始帧一致。同时使用 ffprobe 检查实际 codec、采样率、声道和尺寸，不把版本输出或空文件当作转码成功。
- 从 libpulse 主文件分离 `waveout.rs` 与 `timing.rs`。时间和延迟由实际 [waveOutGetPosition](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/nf-mmeapi-waveoutgetposition) 游标及已接收样本字节数得到，替换墙钟 elapsed 与固定零延迟；遵循驱动返回的时间单位，处理 32 位计数回绕，flush 后重置原生计数并接续流位置。设备查询、暂停和重置失败不再报告成功；录音计时在真实捕获设备接通前明确返回不支持。
- START_CORKED 同时暂停原生设备，flush 后恢复原有暂停状态。补 `pa_stream_ref` 和最终引用释放，计时回调执行期间持有流引用，允许回调释放应用持有的引用。没有读写回调时 uncork 不创建音频 worker；删除无消费者的 KINAKAZE_PULSE_TRACE 环境开关及热路径日志调用。移除输出声道的静默 clamp，交由原生设备接受或明确拒绝请求格式。
- `PulseTimingProbe.c` 使用真实原生设备播放静音样本，验证空流时间为零、暂停不前进、续播前进、延迟下降、flush 后续接和计时回调释放引用；并验证 bad-state / invalid 错误及原输出保持。Pulse 主循环与 ALSA 回归也通过。计数回绕和重置另有 Rust 单元测试。
- 最终固定产物 `artifacts/media-runtime-dist`：**5/5 媒体行为探针通过**；全 workspace **1752 通过、0 失败、24 忽略**，文档测试通过。导出工具 20 项、依赖工具 17 项通过，当前 29 模块 / 4993 导出的一致性检查通过。顶层三项、36 个原生 SO 原字节、空 PATH/无关 cwd、smoke 资源归零均通过。验收汇总 `artifacts/media-runtime-acceptance.json`，各检查报告和二进制哈希一并保留。
- 对相同稳定 ELF 文件范围审计，版本化缺失 **22 → 14**（libc 13、ALSA 1）；这是导入覆盖结果，不等于 Pulse 整套行为已经完成。Pulse 录音仍有旧静音占位数据，server / sink-input 信息与音量、活动流的回调调度和生命周期仍需收敛；本批只验收原生播放计时及离线 FFmpeg 编解码，不宣称全部实时音视频、硬件编码、fork 恢复或性能指标已经达成。继续推进剩余 libc/ALSA、真实采集和完整音频状态，长期 goal 保持 active。

## 2026-09-17 前批：printf 注册、可重入随机数与网络数据库

- 已实现用户指定的 9 个入口：register_printf_modifier/specifier/type、initstate_r/random_r、getprotobyname_r、setnetent/getnetent/endnetent；配套补 srandom_r/setstate_r，并将 parse_printf_format 的占位实现接入真实参数分析。实现分别位于 libc 的 format/extension、random_state 和 netdb；业务状态仍由所属模块管理。
- printf 注册表按 GNU ABI 保存类型、修饰符和回调，支持最长修饰符匹配、混合寄存器/栈参数、多参数处理、真实 FILE、截断计数与错误传播。回调执行时不持注册表锁，传出参数和输出缓冲使用客体内存；注册状态通过 runtime 生命周期接口恢复。普通 fprintf 保留汇总写入，snprintf 自定义输出通过 cookie 直接写入有界目标。真实 libquadmath.so.0 已验证 Q 修饰符、binary128 精度、宽度、混合参数和 fork。
- random_r 实现 8/32/64/128/256 字节五种 glibc 状态算法，状态全部在调用方内存中，支持非对齐存储、种子重置和状态切换。五种首值由独立编译的 glibc C 实现核对；探针覆盖 1000 次生成、种子边界、状态切回以及 fork 后继续生成。
- 协议查询仅使用客体 /etc/protocols；_r 的别名指针表正确对齐，ERANGE 不污染调用方输出。网络枚举与查询游标分离，复用独立的缓冲读取/序列化模块；枚举重置、EOF、关闭重开和 fork 均通过。协议、网络、服务与 ether 非可重入返回对象迁入客体内存，修复 fork 后访问已返回指针的崩溃。
- 新模块注册暴露了 runtime 生命周期表原有 32 项上限；改为按注册需求增长，保留同 key 替换顺序与通知句柄释放，96 项注册/替换测试通过。构建用 ELF 数据定义从零地址绝对符号改为标准数据节，修复 GNU ld 链接 libquadmath 时无法解析 signgam 的版本化数据依赖；运行时继续直接绑定原生 SO 的真实存储，没有增加中转层。
- 本轮固定构建全 workspace **1751 通过、0 失败、24 忽略**，文档测试通过；导出工具 20 项、依赖工具 17 项测试通过。独立产物的 printf/random/quadmath、网络/服务、loader-entry/native-math、Git 4/4 clone、线程栈、锁页、Python、Node、GCC、spawn 和 libc 行为通过。随机数与 quadmath 的首次独立运行在进入客体前遇到 init 会话失败（-2 / pipe 232），单独复跑均通过；原始失败和复跑分别保存在 isolated-matrix / isolated-repeat 报告，没有改写失败记录。
- 验收汇总为 `artifacts/printf-random-net-acceptance.json`，验证目录为 `artifacts/printf-random-net-dist`。共享 portable-dist 曾被并行构建覆盖，因此最后验收使用独立目录；顶层三项、36 个原生 SO 原字节、空 PATH/无关 cwd 和 smoke 资源归零均通过。其他任务随后添加的 Pulse 源码未包含在这套固定映像中，不能将本轮验证扩展为当前所有并行改动已经验收。
- 对前批相同稳定 ELF 文件进行比较，版本化缺失 **31 → 22**（libc 13、Pulse 8、ALSA 1）；新增 Firefox 的更大范围审计单独保留，不混用计数。此构建 FFmpeg 当前缺 pa_stream_update_timing_info@PULSE_0，仍未完成转码。printf 位置参数和完整宽字符格式化等既有边界未在本批解决；没有宣称完整 glibc 兼容或量化性能提升。长期 goal 保持 active。

## 2026-09-17 前批：真实锁页与 FFmpeg 后续依赖

- 新增 libc `memory_lock` 模块，移除 mlock/munlock 的假成功，实现 VirtualLock/VirtualUnlock 和按需 QueryWorkingSetEx；补 mlockall、munlockall、mlock2 及对应原始 syscall。重叠锁不计引用，失败仅回滚新增锁；解除映射由 Windows 清理，fork 新进程不继承原生页锁。复用现有 fork 映射事务，无后台采样或锁页影子表。
- MCL_CURRENT 尝试锁定当前可访问的已提交区域，受原生工作集配额限制；MCL_FUTURE、MCL_ONFAULT 尚未接通所有分配/缺页路径，明确返回 EOPNOTSUPP。munlockall 查询全部已提交区域，包括锁定后改为 PROT_NONE 的页。没有宣称已完整支持实时应用锁页策略。
- 原生 6 项锁页测试直接检查 Windows Locked 位并通过；全 workspace 1749 通过、0 失败、24 忽略，文档测试通过，记录 `artifacts/memory-lock-rust-summary.json`。共享 target 被其他构建/进程使用后，改用 `artifacts/memory-lock-target` 独立构建和测试；导出 20 项、依赖 17 项检查通过。
- 最终 `memory-lock-final-matrix.json` 为 6 通过、1 失败：Git 增强传输 4/4 clone、线程大栈帧/fork、锁页/fork、ALSA、Python、GCC 通过；FFmpeg 仍未执行到转码，当前缺 register_printf_specifier@GLIBC_2.10。产物包含工作区新增的 libm 导出，当前版本化导入审计剩 31 项（libc 22、Pulse 8、ALSA 1）；数学 ABI、精度边界和 signgam 的 COPY/fork 行为仍需专项验收。
- `memory-lock-package-check.json` 验证顶层三项、36 个原生 SO 与独立构建 DLL 原字节一致、空 PATH/无关 cwd 启动；smoke 的资源计数归零。没有新增 SDK、模块清单、中转 DLL 或 runtime 业务状态。下一批按用户要求补 printf 注册与实际调度、可重入随机数、协议与网络枚举接口。长期 goal 继续 active。

## 2026-09-17 前批：Git 传输与原生线程栈

- 原始 `common-git-transfer` 在修复前稳定返回 Python `waitpid` 的 EIO。新现场 `worker.exe.44572.dmp` 的异常是 `0xC0000005`，指令为 `sub rsp,0x10048; mov dword ptr [rsp],esi`；RSP/写入目标 `0xc1cefef8d0`，TEB StackLimit `0xc1ceffd000`，跨越 `0xd730` 字节。提取记录：`artifacts/git-stack-before-dump.json`、`git-stack-before-dump-details.json`。
- 实机裸汇编实验表明：RSP 已进入未提交页时，Windows 可能无法建立异常分发帧，VEH 连入口都未到达；`stack-veh-entry-result.txt` 记录退出 `0xc0000005`、入口计数 0。因此修复不依赖事后 VEH 扩栈，避免把仍会崩溃的恢复路径当作完成。
- `libs/libpthread/src/stack.rs` 独立负责原生栈准备：按真实 VirtualQuery 几何提交可用保留区，移动并保留完整 guard span，留下底部不可访问页，并同步 TEB StackLimit。Windows 仍拥有并释放原生栈；不注册额外映射、不添加环境变量或固定栈容量。GetCurrentThreadStackLimits 返回保留区下界，已提交下界必须读取 TEB。
- pthread 启动握手等待栈/TLS 准备结果；等待前释放 fork 映射事务，避免与 TLS 初始化互锁。失败返回 EAGAIN 并收回线程记录和结果，成功才进入 ELF 入口。重补工作区中只有导出声明而缺失实现的三个调度属性函数，解除链接失败。VEH 的诊断栈读取改用 ReadProcessMemory，避免打印日志时递归访问无效 RSP；没有改变 waitpid 的异常报告语义。
- 增强 `common-git-transfer`：保留原始 clone，并生成多版本相似 blob，额外进行三次 `pack.threads=4` 的 `clone --no-local`；每次验证 HEAD、文件 SHA-256、`fsck --full` 和 `verify-pack` 的实际 delta 记录。`ThreadStackProbe.c` 验证 64/128/256 KiB 无逐页探测的汇编栈帧、red zone、默认/显式栈大小、多线程，以及 192 KiB 活跃栈上的 fork 与子进程继续创建线程。
- 1739 个 Rust 主测试通过、0 失败、24 忽略；记录在 `artifacts/git-stack-rust-summary.json`。文档测试首次遇到共享 crate 元数据错误，独立全 workspace 复跑通过（`git-stack-doctest-recheck.log`），随后重新构建发布产物。导出/依赖检查通过，原生 smoke 的 processes/objects/transactions 均归零。
- 最终产物验收 `artifacts/git-stack-acceptance.json`：Git 增强传输连续 **2/2 轮通过、8/8 次 clone 成功**，每轮同时通过大栈帧与线程内 fork 探针。相关程序矩阵 **11/12 通过**，唯一失败为此前已出现的 nginx SIGQUIT 停机 12 秒超时，保留原始失败，不计作成功；Python、Node、GCC/Clang、Redis、spawn、ALSA/libc 均通过。Java JIT/线程/文件/8 次 ProcessBuilder 及 loader-entry/native-math 的 fork 回归另测通过。发布目录核验通过：仅三个顶层项目，36 个原生 SO 与构建 DLL 字节一致，空 PATH/无关 cwd 启动成功。
- 内存边界：提交可用栈会增加系统 commit charge；代码不逐页写入、不预热工作集，[VirtualAlloc 的 MEM_COMMIT 页面仍按首次访问取得物理内存](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualalloc)。这是运行未经 Windows 栈探测编译的 ELF 所需的正确性处理，不声称 RSS 或吞吐有量化改善。长期 goal 继续 active。

## 2026-09-17 前批：依赖符号、ALSA 与 libc 行为

- 对当前 rootfs 的版本化导入逐项审计：缺失 **187 → 73**。还剩 libc 24、libm 40、Pulse 8、ALSA 1；完整列表在 `artifacts/missing-symbols-verified-audit.json`。这项审计不覆盖所有未版本化符号，X11 独立审计仍有 13 项实际引用缺失；没有宣称所有程序兼容。
- 四个模块导出变化：libasound 98→191、libc 1292→1312、libpthread 110→119、libm 156→166。兼容别名直接复用原实现，不加 DLL 中转层。pthread 屏障、libc 数学分类和历史 libpthread 入口共用实现；屏障注册从泄漏的引用改为 Arc，销毁时检查等待者并回收。
- ALSA 拆为 formats、output、status、polling、mixer、rawmidi/info、midi_event。格式元数据区分 S24_LE 的 4 字节容器和 S24_3LE 的 3 字节容器；播放时处理有符号 8 位与 24 位打包，掩码反映格式约束。PCM 状态按队列帧数查询；writei 受 buffer_size 限制、支持短写，满队列的非阻塞写返回 EAGAIN。waveOut 完成回调唤醒 eventfd，snd_pcm_wait 不再每毫秒轮询。
- snd_pcm_dump 写入真实客体 FILE；输出对象、格式掩码、状态与 info 使用客体内存。mixer 只公布宿主支持的播放音量，查询/设置由 [WinMM volume API](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/nf-mmeapi-waveoutgetvolume) 执行，捕获/开关能力未提供。Raw MIDI 查询宿主输出设备并发送 [WinMM 短消息](https://learn.microsoft.com/en-us/windows/win32/api/mmeapi/nf-mmeapi-midioutshortmsg)，保存部分消息和 running status，使用时检查进程身份后重开句柄。MIDI 编码器支持实时字节穿插、控制事件与有界 SysEx 分片；它不等于原生 SysEx 传输已经实现。
- strtod/strtof 改为 C locale 的 UCRT 解析，修复十六进制、尾指针、范围 errno 及 f32 双重舍入，取消逐前缀重复解析。原生 locale 只在使用时建立，客体 errno 与宿主 errno 分离。补 strtoll_l/strtoull_l、memccpy、ftime、scandirat/versionsort、setbuffer、err/errx；补有限数学别名、exp10f/jnf/ynf 和按位 signaling-NaN 分类，后者包括 x87 与 binary128 参数 ABI。
- 清除 open_memstream 的固定 `/tmp/memstream.tmp` 占位路径，改为通过 cookie 回调访问可增长的客体内存，独立流与 fork 后继续写入均通过。pthread_getcpuclockid 原先误报 CLOCK_REALTIME，现绑定真实线程并由 clock_gettime/getres 读取 CPU 时间。补调度属性存储和校验；不具备的显式 FIFO/RR 保证在创建时返回 EPERM。
- 全量构建日志 `artifacts/missing-symbols-complete-build.log`：**1736 个 Rust 主测试通过，0 失败，24 忽略**，计数见 `missing-symbols-rust-summary.json`；导出、依赖检查与原生 smoke 通过，smoke 清理计数归零。`missing-symbols-verified-matrix.json` 的新增 ALSA/libc 行为探针均通过；FFmpeg 当前缺 mlockall，未运行到音频处理。
- 边界：PCM 同步分组明确不支持；Raw MIDI 输入和原生 SysEx 传输尚未提供；活动 PCM/Pulse 私有资源的 fork 恢复、Pulse 录音旧占位路径和回调归属仍待完善。尚未测量 RSS、吞吐或延迟，不宣称量化性能提升。长期 goal 保持 active。

## 2026-09-17 前批：指针控制、窗口树和 X11 核心接口

- X11 351→402 个导出；新增 pointer_control、window_tree、colormap、accessors、text、icon_sizes 等模块。XChangePointerControl 接真实 SPI_GETMOUSE/SPI_SETMOUSE，保留 Windows 两个默认阈值；原生加速档位会量化 X11 比率，查询返回实际读回值。测试具备 finally 恢复宿主设置的 guard。
- 窗口重挂接走窗口所属 UI 线程，检查循环并保持客户区尺寸；修复 Windows 最小跟踪尺寸把 64×48 子窗口放大到 120×48 的问题。TrueColor、标准 colormap 属性、字符串列表和图标尺寸使用真实状态及客体所有权；相应模块按需序列化自己的 fork 状态。XNextEvent/XPeekEvent 共用每个等待者自己的 eventfd，取消旧的 2ms 轮询。
- `x11-controls-final-build.log` 的 1732 个 Rust 主测试通过；72 项工具矩阵 67 通过、5 失败。四个新增 X11 探针和增强事件选择探针通过。失败包括本批新增 locale 十六进制解析（已在最新批次修复）、FFmpeg 的 ALSA 缺口、Git 传输及 nginx 测试。完整记录保留在 `x11-controls-tool-matrix.json`。
- X11 参考库 1238 个导出中当前实现匹配 400 个；当前 rootfs 真正引用且缺失的接口 32→13。字体加载、完整 XKB、背景 pixmap、活动抓取、异步协议读取与其他原有占位实现仍待补齐，不能将字段 ABI 探针视为完整 X server 行为验收。

## 2026-09-17 前批：正常 ELF 退出、X11 属性和字体尺寸

- 本批新增 **11 个 X11 ABI 入口**：屏保重置；文本属性 set/get；WM hints 查询；窗口/图标名称查询；8/16 位字符的尺寸与宽度；窗口背景色。并替换原先空的属性读写、删除、WM hints 设置和错误的预定义原子编号。实现按职责放在 `property.rs`、`property/{atoms,hints,lifecycle}.rs`、`font_metrics.rs`、`screensaver.rs`。
- `launch::enter` 现在在 RDX 交付 ld.so 的 rtld_fini。loader 解析并保存逆依赖顺序的 DT_FINI_ARRAY/DT_FINI 队列，每个回调在调用前从队列移除；调用期间不持有 loader 锁或元数据引用。队列由 loader 自己序列化，fork 后重新取得子进程的 linker owner。析构调度参考 [glibc dl-fini](https://raw.githubusercontent.com/bminor/glibc/master/elf/dl-fini.c)，没有向 runtime 添加装载业务。
- libc 将退出回调链拆至 `process/termination.rs`，独立 lifecycle 只传递回调、参数和 DSO 值，在子进程重建 Vec 和锁；runtime 已校验 native 模块装载基址，客体映射保持原地址。注册失败会被报告，启动阶段不忽略 rtld_fini 登记失败。`ExitLifecycleProbe.sh` 验证 main/依赖 DSO 顺序、重复及重入 __cxa_finalize、回调中登记、递归 exit、_exit 不析构、普通 fork、atexit 内 fork、DT_FINI_ARRAY 内 fork 及准确退出码。
- gcov 无需显式 __gcov_dump，正常返回后已经生成 branch.gcda，gcov/gcov-dump 的行、分支执行和分支覆盖均通过。早先显式 dump 仅为定位；本批 `artifacts/exit-loader-probes.json` 中 gcc-runtime、binutils、gcov、退出生命周期四项均通过。
- 属性以紧凑的 8/16/32 位数据私有存储；格式 32 对外按 Linux long 扩展并符号扩展，长度及 offset 使用协议的 4 字节单位。实现替换、前插、追加、局部读、类型不符、bytes_after、读完删除及错误回调；返回值用客体内存并额外 NUL 终止，类型不匹配也返回可释放的空缓冲区。依据 [X.Org GetProp](https://raw.githubusercontent.com/mirror/libX11/master/src/GetProp.c) 和 [LP64 读取实现](https://raw.githubusercontent.com/mirror/libX11/master/src/XlibInt.c)。
- 使用标准 68 个预定义原子，动态原子从 69 开始，only_if_exists 不创建新值，名称返回客体内存。原子和属性由模块快照恢复，返回对象由调用者 XFree；WM hints、class/size hints 的分配也迁到客体分配器，去除原来的 Rust Box 外露。XPropertyProbe 覆盖格式、边界、错误、文本/WM 数据和 fork 后独立状态。
- 字体尺寸计算直接使用调用者的 96 字节 Linux XFontStruct，不分配内存；支持单行/多行字符表、缺字默认字符、空串、bearing/ascent/descent 和大于 short 范围的宽度。它不代表已有占位字体加载器已经完成。背景色接到现有绘图存储，DBE 探针检查实际清屏像素。
- XResetScreenSaver 只在请求时检查 Windows 合成输入重置策略，并发送无位移的活动通知；策略禁止或 SendInput 失败时返回失败。不改屏保配置，不创建周期线程；[Windows 策略接口](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfow)。实际屏保激活/超时场景尚未验收，XGet/SetScreenSaver 旧路径仍待替换。
- 最终完整构建 `artifacts/exit-property-complete-build.log`：**1730 Rust 主测试通过、0 失败、24 忽略**，20 导出与 17 依赖检查通过，smoke 退出后进程/对象/事务为 0。此前窗口检查误拒后端直接创建的 HWND，已修正为真实窗口有效性检查并完整复跑；统计见 `exit-property-rust-summary.json`。
- `artifacts/exit-property-tool-matrix.json` 的完整 **68 项为 65 通过、3 失败**，其中 56 项行为和 9 项启动通过；新增退出、属性、字体尺寸、背景像素，以及 gcov 均通过，Node/Python、nginx 双 worker、Redis、GCC/Clang、binutils 与 Pulse 主循环均复测通过。剩余为 FFmpeg 的 XChangePointerControl、Git 传输真实子进程异常、nginx TLS 证书阶段 OpenSSL 空 thunk；本报告完成后开始下一批指针与窗口接口，不混用不同构建的结果。
- `artifacts/exit-property-package-check.json` 通过顶层三项结构、36 个 MZ 映像与构建 DLL 原字节一致、独立静态入口，以及空 PATH/无关 cwd 启动。

边界：尚未完成 X11/XInput/音频整组；FFmpeg 下一处缺失是 SDL2 的 XChangePointerControl，真实转码尚未通过。属性通知和多进程共享 X server 语义未实现，现有 XCB 仍持有独立的旧原子状态；不得将 fork 私有快照视为跨进程 X server。字体加载、完整 XKB、Pulse 录音及其活动状态恢复仍需推进。未运行性能基准，不宣称吞吐/RSS 改善。长期 goal 保持 active。

## 2026-09-17 前批：Pulse 主循环、XIM、X11 事件筛选与开发工具

- 新增 **29 个公开 ABI 入口**：Pulse 15 个、X11 14 个；并替换原先线程主循环的假 API 指针、XIC 固定句柄、无结果的属性查询和借用原字符串的文本转换。实现分别位于 `libs/libpulse/src/mainloop/`、`threaded.rs`、`libs/libX11/src/input_method/`、`event_select/`、`keyboard.rs`、`text.rs`；没有增加模块清单、SDK、中转 DLL 或 runtime 业务状态。
- Pulse 提供真实的 112 字节 `pa_mainloop_api`，覆盖 poll 的 prepare/poll/dispatch、Unix 绝对时间定时器、延迟回调、once、退出和唤醒。调用者的自定义 poll 收到实际客体 FD 与毫秒超时；微秒转换向上取整，defer 优先于 poll/timer/I/O。FD/就绪数组复用容量，eventfd 合并唤醒，不添加空闲采集线程。返回及调度规则对照 [Pulse 官方实现](https://raw.githubusercontent.com/pulseaudio/pulseaudio/master/src/pulse/mainloop.c)。
- 线程主循环使用真实调度线程；poll 期间释放递归锁，回调阶段重新获得锁，保留 wait/signal/accept 的完整握手。stop 会唤醒并回收线程，清理尚未 dispatch 的迭代状态，支持再次 start。事件回调在内部表锁之外执行，可删除其他就绪事件；单调事件 ID 防止释放后地址复用误触发新事件。
- Pulse 的 native Box/锁/线程由本模块负责，活动主循环或 once 暂拒绝 fork（EAGAIN），全部释放后允许 fork，不复制宿主堆。`PulseMainloopProbe.c` 实际验证管道、Unix socket EOF、定时器重启/取消、回调删除、定制 poll、跨线程唤醒、递归锁、signal/accept、stop/start 和 fork 边界。
- 本地 XIM 上下文在客体 arena 中构成所有权链；上下文和属性可以随 fork 恢复，子进程修改/释放不影响父进程。SysV 可变参数入口同时处理寄存器和栈参数；支持样式、客户窗口、焦点窗口、filterEvents 查询以及 mb/UTF-8/wide 的直接按键输出、Shift/Caps/Control、容量不足状态和 reset。仅公布无预编辑/状态 UI 的直接键盘输入模式；不宣称中文组合输入、远程 IM 或任意键盘布局已完成。
- XQueryKeymap 按调用读取 Windows 按键状态，再沿现有 scancode→evdev→X11 映射生成 32 字节结果，不缓存采样。文本列表转换进行两遍长度/编码处理，仅分配最终客体缓冲区，保留列表内的 NUL 分隔和末尾 NUL；修正 STRING 的标准原子号为 31。支持 UTF-8 与可表示的 Latin-1；COMPOUND_TEXT 及不能支持的转换明确返回 XConverterNotFound。
- XWindowEvent/XMaskEvent 和 check 系列保留未匹配事件，XPutBackEvent 真正放回队首，QLength 随队列同步。阻塞消费者各自拥有一个按需 eventfd，其他线程的 XPending 或事件读取不会抢走它的唤醒；空闲时无轮询。掩码与移动事件按钮筛选依据 [X.Org 事件匹配实现](https://raw.githubusercontent.com/mirror/libX11/master/src/WinEvent.c)。活动等待帧暂拒绝 fork，回收后允许 fork；不增加周期监控。
- `XimProbe.c` 验证 XIC 属性、可变参数、字符/宽字符容量边界、文本所有权、键图边界和 fork 独立状态；`XEventSelectProbe.c` 验证队列保留、按窗口/类型/掩码筛选、移动按钮状态以及两个消费者与 XPending 并行的 16 轮唤醒。
- 新增 binutils 行为探针：ar/ranlib 建静态库，GCC 链接运行，nm/readelf/objdump/size/strings 检查真实程序，objcopy 分离调试文件并添加 debuglink，strip 后再次执行，addr2line 映射到源码，c++filt 解码与 elfedit 修改后执行。`artifacts/developer-tools-probes.json` 的 binutils 已通过。
- 新增 gcov 正常退出验收，当前仍失败：程序返回 0，但没有生成 branch.gcda。独立诊断 `artifacts/gcov-dump-diagnostic.json` 显式调用 __gcov_dump 后能产生文件，gcov/gcov-dump 解析成功，分支执行/覆盖均为 100%；提前 dump 时最后一行尚未执行，因此行覆盖 87.5%。诊断不替代正常退出验收。代码证据：`launch::enter` 把 rtld_fini 的 RDX 清零，libc 只登记传入的 fini/rtld_fini，普通 _start 路径从不返回到 host 的 run_finalizers。本批定位到析构衔接缺口，尚未修改 loader/exit 生命周期。
- 最终完整回归 `artifacts/pulse-x11-verified-build.log`：**1728 个独立 Rust 主测试通过、0 失败、24 忽略**；20 项导出检查、17 项依赖检查通过，原生 smoke 的进程/对象/事务回到 0。计数见 `pulse-x11-rust-summary.json`，没有重复计入辅助测试进程输出。
- `artifacts/pulse-x11-package-check.json` 检查发布顶层仍仅 init.exe、worker.exe、rootfs；36 个 MZ 文件与构建 DLL 字节一致，入口来自静态入口构建，空 PATH/无关 cwd 可以启动。没有新增生产环境变量。
- 最终 65 项工具矩阵 **61 通过、4 失败**（52 项行为、9 项启动通过），全部使用本批最终产物；报告为 `artifacts/pulse-x11-tool-matrix.json`。新增 Pulse、XIM/文本、并发事件筛选与 binutils 探针全部通过。FFmpeg 已通过本批新增入口的解析，当前仍缺 SDL2 使用的 XResetScreenSaver；其他失败为 Git 传输、nginx TLS 证书生成、gcov 自动退出落盘。Node/Python、GCC/Clang 运行、普通 nginx 多 worker/并发/重载、Redis 持久化、Unix socket 句柄传递、rsync daemon 和前批图形探针均通过；PostgreSQL/Java/SSH 在此矩阵中的条目仅为启动，不替代先前独立行为验收。
- 边界：整组 X11/XInput/音频仍未完成；Pulse 录音仍有旧占位路径，不能作为实际采集能力验收，需继续替换；流回调线程归属、私有 RTCLOCK 定时器、活动音频资源 fork 恢复、XIfEvent 谓词队列语义及窗口属性等继续补齐。当前只报告实测行为，尚无 RSS/吞吐对比，不宣称量化性能提升。长期 goal 保持 active。

## 2026-09-17 前批：Xdbe 双缓冲与 X11 事件转换

- `libXext.so.6` 新增完整一组 9 个 Xdbe ABI 入口，协议代码在 `libs/libXext/src/dbe.rs`，生命周期在 `dbe/lifecycle.rs`；X11 的 `graphics/buffered.rs` 只负责窗口关联的绘图表面及资源释放，没有增加中转 DLL、清单或 runtime 业务逻辑。
- 同一窗口的多个 back-buffer 名称共享一份像素；Undefined/Untouched 交换表面所有权，Background 清理新的后缓冲，Copied 保留新前缓冲的副本。交换直接借用调用者数组，不构造中间 Vec；整个批次持有呈现表面的锁，呈现线程不会看到半批交换。无活动缓冲时仅检查原子计数，不添加后台采集。
- XClearWindow/XClearArea 同步清理两个缓冲；宽/高为 0 延伸至窗口边界。未实现 bit gravity 时缩放清理整个表面，已有窗口 resize 路径发布 Expose。XGetGeometry 支持 pixmap/back-buffer 的尺寸和深度，保留根窗口查询；back-buffer 不能作为 Window 查询属性。窗口销毁和显示关闭释放名称及表面。语义依据 [X.Org DBE 规范](https://xorg.freedesktop.org/archive/X11R7.7/doc/libXext/dbelib.html)。
- 修正 XCreateSimpleWindow 漏传背景属性 mask，以及核心 GDI 绘图将 X11 RGB 像素误作 COLORREF 导致的红蓝通道交换。新增 `XdbeProbe.c` 验证初始背景、前后缓冲独立、4 种交换、多名称、多窗口、CopyArea、清屏、缩放、错误返回与回收。
- `event_wire.rs` 新增 XESetEventToWire/XESetWireToEvent 和 `_XEventToWire/_XWireToEvent/_XEnq`；回调在注册表锁外执行，支持替换和返回旧回调，NULL 安装拒绝转换回调。核心编解码覆盖键盘、按钮、移动、焦点、Expose、ConfigureNotify 和 ClientMessage 的 8/16/32 位负载；XSendEvent 现在实际入队，编码失败返回失败，接收回调可过滤事件。实现参考 [X.Org 回调注册](https://raw.githubusercontent.com/mirror/libX11/master/src/InitExt.c) 与 [发送路径](https://raw.githubusercontent.com/mirror/libX11/master/src/SendEvent.c)。
- 回调表通过 runtime participant 显式序列化，内建函数在子进程重新绑定，新建同步锁；不复制 Rust map 或宿主堆。事件转换回调内部 fork 与活动 DBE 表面的 fork 暂返回 EAGAIN，释放后可 fork；客体查询数组可正常随 fork 恢复。`XEventWireProbe.c` 验证字节负载、负坐标、回调重入、过滤、子进程恢复及显示关闭。
- 补齐锁定 OpenSSL 包原有的 `etc/ssl/openssl.cnf` 和 `usr/lib/ssl/openssl.cnf` 别名；二者均验证 Debian 归档中的实际目标与 SHA-256，没有注入 OPENSSL_CONF。`NginxProxyProbe.py` 新增 TLS、Unix-domain 上游、压缩和连接复用的验收场景，但目前在证书生成阶段失败，尚未执行到 nginx TLS，不能记为已支持。实际首个异常已捕获：旧 OpenSSL 3.0 命令进入 rootfs 的 3.6.4 `libcrypto`，在 `OPENSSL_LH_doall` 调用空 thunk（返回地址为 +0x70）；随后 VEH 报告器发生不能跨 ABI 展开的 Rust panic，表现为 `0xC0000409`。证据为 `openssl-null-debugger.log`、`openssl-crash-stack.json`。这条异常报告路径本批仅诊断，未修改。
- 已审计全部 SSL 消费者的版本需求，记录在 `artifacts/openssl-dependency-audit.json`。curl 和 ngtcp2 需要 OPENSSL_3.5.0，不能直接把整套库降为锁定的 3.0 来绕开证书故障；需要统一可用的真实依赖组合并复测。当前仅配置文件已补齐，SSL ELF 文件未替换。
- 最终构建 `artifacts/dbe-events-final-build.log`：1725 个独立 Rust 主测试通过，0 失败、24 忽略；另有 20 项导出检查和 17 项依赖检查通过，原生 smoke 的进程/对象/事务均回到 0。清屏新增确定性测试，覆盖窗口在范围查询之后缩小时按实际表面重新裁剪。
- 基础 59 项矩阵为 57 通过、2 失败；加入 nginx TLS 扩展探针后，总验收 60 项为 **57 通过、3 失败**（48 项行为、9 项启动通过）。失败分别是 FFmpeg 的 Pulse 主循环、Git 传输、证书生成阻塞的 nginx 扩展探针。配置更新后单独复测 Python、Node、普通 nginx、OpenSSL 基础行为均通过，17 项依赖检查也重新通过。汇总 `artifacts/dbe-final-tool-results.json` 保留两份原始报告、二进制哈希和依赖变更记录。
- `artifacts/dbe-events-package-check.json` 确认最终目录仍只有 init.exe、worker.exe、rootfs；36 个 MZ 文件与原始 DLL 字节一致，入口与静态构建一致，空 PATH、无关 cwd 启动通过。没有新增生产环境变量或后台采集；长期 goal 保持 active。
- Git 传输保留了可复现诊断：`artifacts/git-process-tree.json` 记录真实 exec 工作进程以 `0xC0000409` 结束，外层 fork/exec 包装进程为 0；`git-process-tree.stdout.log` 显示最后停在启动 git-upload-pack。`waitpid(EIO)` 是缺失客体退出报告的后续表现，不能据此猜测 Linux 退出码或宣称只是 wait 接口问题，根因继续定位。诊断只在该探针运行时观察它的进程树，不增加生产后台监控。
- 边界仍明确：窗口/GC/活动原生表面的完整 fork 恢复、一般事件掩码/传播路由、其余 wire 事件及整组图形/音频接口尚未完成；不支持的事件发送路由返回 BadImplementation。FFmpeg 已通过 Xdbe 与事件转换符号解析，当前阻塞移动到 SDL2 的 `pa_mainloop_new@PULSE_0`。Pulse 主循环需要实际 poll/timer/defer 与真实 `pa_mainloop_api`，不能用返回空指针或把同步对象强转成 API 表来代替。没有吞吐或 RSS 对比数据，不宣称量化性能收益。

## 2026-09-17 前批：Xext 独立 SO、共享图像、rsync 与进程状态竞态

- `libXext.so.6` 从 X11 导出转发层变为自己持有实现的原生 SO；Xext 列表、SHAPE 边界和 MIT-SHM 分别在 `libs/libXext/src/extension.rs`、`shape.rs`、`shm.rs`。构建产物继续直接复制 DLL 字节为 SO，没有清单、SDK 或 implementation 子层。
- 新增 10 个 XShm 导出。共享图像通过真实 SysV 段建立独立的服务端附件，校验偏移、步幅、尺寸和读写权限；实现像素写入/读回、完成事件与 detach。共享图像析构只释放 XImage 头，不释放 shmat 映射；普通 XDestroyImage 尊重图像自己的析构函数，XPutImage 不再覆盖它。
- XGetImage 与 XShmGetImage 共用 `libs/libX11/src/image/readback.rs`，直接写入最终客体缓冲区；小区域读回不再复制完整源表面。32 位小端像素使用逐行复制或掩码写入，其他格式沿用像素操作。drawable 尺寸查询不再构造像素 Vec。尚未做 RSS 或吞吐对比，不能将这些代码变化换算成实测收益。
- 修正 XExtensionInfo 的 LP64 布局、链表查找/删除、用户 data 保留和缺失扩展的 NULL codes。列表及其代码块使用客体内存；模块只向 runtime 序列化根指针，在子进程中使用新锁。显示关闭回调允许重入和移除自己的记录；不能分发的 wire/GC/font 钩子拒绝注册。
- libc 增加 `fallocate/fallocate64/lchmod/getpass`。文件预分配归入 VFS，持有固定的文件描述，保留既有内容/较大的预留空间；KEEP_SIZE 的原生测试查询实际 Windows AllocationSize 与 EOF。getpass 使用控制终端、关闭回显并恢复 termios，复用且清零客体密码缓冲区，通过本模块的 fork participant 恢复地址与容量。
- rsync 已通过本地目录复制、2 MiB 数据、空格文件名、硬链接/符号链接、预分配、内容校验更新和删除；独立 daemon 探针通过 TCP 认证上传、zstd 压缩、更新删除、下载和错误密码拒绝。关闭 chroot 时服务端符号链接按 rsync 默认规则加保护前缀，下载恢复原始链接；依据 [rsync 官方说明](https://download.samba.org/pub/rsync/rsyncd.conf.5)，修正了最初测试对服务端链接的错误预期。
- 修复进程暂停的实际竞态：进程表重建哈希索引会短暂清空索引，无锁 state/has_pending 查询此前可能漏掉仍然存活的槽位。现在从权威槽位读取，并使用单项线程缓存保持信号热路径为常量时间；不会等待可能已被暂停的索引写线程。新增确定性回归覆盖索引清空期间的冷/热查询和槽位更换。
- Unix listener 故障测试改为在独立进程等待资源所有者的清理屏障，检查残留的远端端点句柄及清理池均为 0。全进程 GetProcessHandleCount 包含线程池和信号观察线程的异步资源，隔离后仍出现 ±1 波动，因此不再用它代替 listener 自己的生命周期断言；没有增加生产路径计数器或后台采集。

最终完整回归见 `artifacts/xext-rsync-state-build.log`：**1722 Rust 主测试通过、0 失败、24 既有忽略**；汇总 `xext-rsync-rust-summary.json` 按 Cargo 测试组计数，辅助子进程输出不重复计入。20 导出测试、17 依赖测试全部通过；smoke 最后进程/对象/事务均为 0。此前三次回归失败记录保留，用于说明暂停竞态和句柄测试修正过程。

`artifacts/xext-rsync-package-check.json` 验证顶层仅 init.exe/worker.exe/rootfs、36 个原生映像与构建 DLL 字节一致、入口来自静态入口构建，且空 PATH/无关 cwd 可以启动。Xext 图像探针验证共享像素往返、完成事件、只读拒绝、越界拒绝、子区域平面掩码读回、析构所有权以及 fork 后扩展列表/关闭回调独立；最终完整 57 项工具矩阵 `artifacts/xext-rsync-tool-matrix.json` 为 **55 通过、2 失败**（46 项行为、9 项启动通过），直接使用本次最终构建。nginx 双 worker/并发/Range/重载/优雅退出、Node/Python、GCC/Clang、Make/Ninja、SQLite/Redis、Unix socket 句柄传递和全部新增探针均通过。两个失败仍为 FFmpeg 的 Xdbe 导出和 Git clone 的 waitpid EIO；PostgreSQL 在此矩阵中仅验证启动，不代表初始化/事务已通过。

边界：SHAPE 和 DBE 尚未实现，FFmpeg 仍报 `XdbeDeallocateBackBufferName` 缺失；整组 X11/XInput/音频仍不能宣称完成。共享 pixmap 不广告支持，活动 XShm 附件的 fork 恢复暂返回 EAGAIN；XYPixmap 的部分平面紧凑读回不支持，返回 BadMatch。图形窗口/GC 与音频设备的完整生命周期、Pulse 主循环/录音和 Git 传输继续推进，长期 goal 保持 active。

## 2026-09-17 前批：XInput/RandR 拆分、图像与音频 ABI、nginx 和常用工具

- `libXi` 与 `libXrandr` 从 X11 转发层改为各自拥有实现的原生 SO，代码直接位于对应 `libs/<模块>/src/`。RandR 从 X11 删除；没有增加运行时清单、SDK 或 implementation 子层。
- XInput 提供虚拟主键鼠和 Windows raw-input 设备的查询、掩码选择/读取与 client pointer；设备、cookie 和返回数据使用客体内存。选择状态通过本模块的 runtime fork participant 序列化；请求临时数组在栈上，查询不再构造临时 Vec，原始事件筛选只读取原子掩码。XI 2.0 不宣称支持触摸或 pointer barrier；调用这些操作会交付真实 X 错误。
- Xlib 实现 XImage 创建/初始化、位图与 8/16/24/32 位像素、子图、像素增量、DIB 读写与像素格式查询；校验步幅及相反位序/字节序的末尾单元边界。GC 初始化与修改按掩码执行；错误处理器可重入，回调地址通过 X11 自己的 runtime 接口恢复。
- RandR 的当前主屏幕尺寸、刷新率和物理尺寸取自 Windows；不再编造 1920×1080、像素时钟或同步时序。查询块及 gamma 数组由客体单块内存持有；gamma 读取/设置接到真实 GDI。完整模式切换和 RandR 事件尚未实现，仍不广告扩展可用；只对已经处于请求状态的设置返回成功。
- ALSA 修正 Linux long/帧数为 64 位，hw/sw 参数改用客体分配器，补入参数约束、复制、声道映射和设备提示；capture 明确返回不支持，不再返回伪造的静音数据。Pulse 拆出声道布局与音量运算，覆盖 AIFF/ALSA/AUX/WAVEEX/OSS 的初始化/扩展/校验和 51 个位置名称，格式化不分配临时字符串。
- libc 搜索树独立为 AVL 实现，修正 tsearch/tfind 返回节点地址，补入 twalk_r/tdestroy；新增 reentrant hsearch 表，节点与表都在客体内存。LP64 有符号/无符号解析共用独立模块，修复高位无符号数被截断以及 LONG_MIN 误报溢出；这已修复 Ninja 的命令哈希误判与无改动重复编译。补入 clearerr_unlocked、checked pread、glob_pattern_p、error_at_line 的 System V 可变参数路径，并接通 GNU 正则的语法选择。error 系列尚不导出 GNU 可写诊断全局变量；正则 AST 的私有堆 fork 恢复仍未覆盖。
- libm 增加 Bessel、exp10、significand、logb、scalb、lgamma_r 及 ABI 同义入口，复用现有数学库；测试检查次正规数、整数边界、指数、符号与浮点异常。
- 锁定依赖累计 313 包，加入 nginx、X11/音频开发头文件、Make/Ninja/pkgconf、Git、sed/grep/find/patch、jq、OpenSSL、rsync、file 与 procps；当前安装的新增工具闭包检查 899 条 ELF 依赖边。file 的 magic 数据和开发头文件依赖显式记录在包依赖中。包被安装不等同程序通过行为验证。
- 补上遗漏的 native helper 启动链：worker → runtime 的版本化 C ABI → engine 内部入口，init 使用独立 Helper 角色认证并管理 Job。辅助进程不分配 Linux PID，也不能调用管理/客体状态接口；现有 host 会话信息继续沿用，没有增加环境变量或常驻采集。修复 nginx 多 worker 通道 SCM_RIGHTS 传递失败，并补入 AF_UNIX 的 SO_DOMAIN/SO_PROTOCOL 供 Python 从接收句柄重建 socket。usernet broker 入口也已接通，尚未单独完成客体端到端验收。
- 汇编 .globl 声明现在纳入导出漂移校验，避免实现已链接但没有 PE 导出的情况；error_at_line 的真实客体可变参数、退出码测试已通过。新增 GNU argz_create_sep，使用单块客体内存；空字符串和连续/末尾分隔符有行为覆盖，语义参考 [glibc 源码](https://raw.githubusercontent.com/bminor/glibc/master/string/argz-ctsep.c)。
- 修复完整测试打包使用了动态标准库入口的问题：测试发布也直接选用独立静态入口构建，测试编译后恢复普通 target 目录的入口副本。

最终完整回归 `artifacts/media-argp-final-build.log`：**1722 Rust 通过、0 失败、24 既有忽略；20 导出测试与 17 依赖测试通过；smoke 结束后进程/对象/事务全为 0**。此前 `media-tools-final-build.log` 曾出现 stop/continue 测试读到 Continued 的竞态；独立五次复测和后续完整回归通过，但没有证据证明根因已修复，保留 `artifacts/media-stop-continue-repeat.json`。

`artifacts/media-acceptance-matrix.json` 的完整 53 项为 **49 通过、4 失败**（通过项为 40 项行为、9 项启动）。MediaAbiProbe、搜索树/哈希表与错误回调 fork、Make/Ninja 增量编译、pkgconf、Git 基础仓库、sed/grep/find/patch、jq 数学与 JSON、OpenSSL、file、ps/free 都通过。nginx 完成双 worker、GET/HEAD/Range/404、2 MiB 文件校验、24 并发请求、SIGHUP 重载和 SIGQUIT 退出，日志无 alert/crit/emerg；独立 UnixRightsProbe 验证发送者退出后的文件句柄存活、共享文件偏移、CLOEXEC 和跨进程 socket 端点转交。

后续 locale 收敛补上 GNU `__dcgettext`/`__stpcpy` 导出别名，并修正 argp 的 Linux 标志位、FINI 常量、位置参数消费和 `--` 语义；不能把已有 argp 实现视为完整 GNU 参数重排/子解析器实现。`artifacts/media-argp-final-probes.json` 的 locale-command、search-runtime、media-abi、nginx-runtime、unix-rights 五项全部通过。汇总证据 `artifacts/media-acceptance-summary.json` 为 **53 项中 50 通过、3 失败**（41 项行为、9 项启动通过）；该汇总保留完整矩阵和定向复测来源，不表示最后一次小修改后重新跑了全部 53 项。

`artifacts/media-tools-sshd.json` 再次完成真实密钥登录、远程管道和退出码 23；测试 sshd 随后退出，没有留下常驻服务。`artifacts/media-tools-java.json` 完成默认 JIT、8 线程、UTF-8 文件以及 8 次 ProcessBuilder 子进程，使用与源码哈希匹配的已有 class；测试 class 和空测试目录随后删除。`artifacts/media-tools-package-check.json` 检查顶层三项、36 个原生映像与构建 DLL 字节相同、入口使用独立静态构建，以及空 PATH/无关 cwd 启动。

剩余工具失败：FFmpeg 首个缺失导出是 SDL2 的 `XdbeDeallocateBackBufferName`；rsync 仍缺 `fallocate`、`getpass`、`lchmod`；Git init/commit/diff/fsck 通过，但 `clone --no-local` 的子进程等待出现 EIO，未将传输计为通过。

边界：X11/XInput/音频接口整组尚未完成；XPutImage 当前仅支持 GXcopy 且不支持 clip mask，XGetImage 当前仅支持 ZPixmap，图形窗口/GC 和音频设备句柄的完整 fork 生命周期尚未覆盖。FFmpeg 当前仍被 SDL2 的 Xdbe 接口阻断；仅参数和 ABI 探针通过不代表录音、所有音频后端或 FFmpeg 可用。未做 RSS、fork 延迟或 Unix socket 吞吐基准，代码减少临时分配不作为性能收益量化结论。长期 goal 继续 active。

## 2026-09-17 前批：真实 locale、字符转换与 PostgreSQL 启动

发布结构继续保持 `init.exe`、`worker.exe`、`rootfs/`。没有新增 SDK、模块目录清单或运行时转发层。

- libc 的 locale 选择、GNU 数据解析、宽字符分类/映射、语言信息和 fork 生命周期归到 `locale.rs` 与 `locale/`。支持 C/POSIX 与 C.UTF-8/C.utf8；不接受未安装名称后静默替换。补齐 PostgreSQL 缺少的 13 个 `_l` 导出，以及对应分类、变换、复制/释放和线程选择接口。
- C.UTF-8 使用锁定 Debian `libc-bin` 中的真实 LC_CTYPE 文件，首个请求才读取、校验目录/偏移/三层表并发布一个不可变客体分配。默认 C 不读文件，也不进行后台采集。模块快照只转交分类位、数据地址和调用线程 locale；pthread 继承借用选择，fork 重建私有同步状态。包锁共 276 项，未以 Debian loader 覆盖原生模块。
- 删除 strextra 中重复或空的 locale/字符实现；字符串转换移到 `uchar/strings.rs`，共用校验过的标量转换器。C 拒绝非 ASCII，UTF-8 拒绝无效序列和代理项。补上 mbstate、NULL sizing、短输出、源指针与 errno 行为；宽字符流在首次定向时保存编码，切换 locale 和 fork 不改变其编码。
- 补入经过包 SHA-256 校验的 Debian tzdata 和真实 locale 命令。zoneinfo 的 POSIX 目录别名从归档中的真实目标展开；系统配置 `/etc/localtime` 不由包准备覆盖。locale 命令仍缺 `error_at_line`，未计为通过。
- 修正 `nl_langinfo` 的 CODESET/日期名称/格式和完整 GNU `lconv` 布局；GNU `newlocale(1 << LC_ALL, ...)` 旧掩码形式按实际语义展开，修复 libstdc++ 初始化异常。为 GNU locale 工具补齐 `_libc_intl_domainname` 的真实 5 字节数据对象和 COPY 布局，版本需求从真实 ELF 记录。定位期间的 abort 回溯已删除。
- 修复非 root exec 的 mount namespace 恢复：继承恢复核对共享进程表已记录的成员关系后重开 namespace，不再调用特权 setns 路径；不同 namespace 的恢复帧拒绝且不改变现有成员关系。Python 探针覆盖降权、完整 UID/GID/附加组、再次 fork/exec，以及实际 setns 仍返回 EPERM。
- `LocaleRuntimeProbe.c` 实测 Unicode 简单映射、汉字/组合字符宽度、非法字符、线程继承/隔离、句柄复制/销毁、分类组合、字符转换以及流编码的 fork 恢复；解析器测试拒绝截断、非法移位和越界偏移。独立无 locale 数据的 ABI 探针改为验证真实默认 C 语义；UTF-8 状态测试在新探针中保留。

最终完整构建 `artifacts/locale-membership-final-build.log` 已通过：1713 Rust 测试通过、24 既有忽略，18 导出与 17 依赖测试通过；smoke 退出后进程/对象/事务归零。此前 locale 单独完整回归为 1712 通过；增加的 1 项验证 namespace 恢复不能切换成员关系。最终工具矩阵 `artifacts/locale-final-tools.json` 为 32 通过、2 失败；`artifacts/locale-final-sshd.json` 验证真实密钥登录、远程管道与退出码 23；`artifacts/locale-final-java.json` 验证默认 JVM JIT、线程、UTF-8 文件及 8 次 ProcessBuilder。`artifacts/locale-final-package-check.json` 验证三项发布布局、36 个原生映像字节以及空 PATH/无关 cwd 启动。

当前边界：PostgreSQL `--version` 和 `initdb --version` 已通过，不能据此视为数据库可用；降权 Python 子进程的 ENOEXEC 已定位并修复为 mount namespace 继承恢复问题，initdb 已完成 bootstrap，后续 VACUUM FREEZE 的 pg_xact 目录打开仍失败，未达到 SQL 服务/重启验收。FFmpeg 的 XInput/SDL 等接口缺口仍需处理。仅覆盖 C/C.UTF-8 locale，不包含任意 locale archive 或全部 glibc 私有 locale 数据接口；libc `_dl_find_object` 尚为空实现，C++ 异常展开仍有缺口。尚未测量 RSS、fork 延迟和 Unix socket 吞吐收益，长期 goal 保持 active。

## 2026-09-17 前批：stdio 生命周期、spawn 与 Clang 实际编译

发布目录保持 `init.exe`、`worker.exe`、`rootfs/`，没有新增 SDK、清单或运行时中转层。确认 `crates/shim-sdk` 无任何消费者后，删除该旧 provider 框架以及 workspace/锁文件条目；保留真实模块已经使用的 runtime API。

- stdio 按职责拆出 `stdio/cookie.rs`、`stdio/wide.rs`、`stdio/lifecycle.rs`。实现 GNU fopencookie 的 System V 回调、读写/seek/close、缓冲、短写、错误传播与关闭一次语义；`fflush(NULL)` 刷新所有流并返回失败，注册表锁不跨用户回调持有。
- 对外 FILE 控制块使用客体分配器，模块私有 Vec/Box 与锁不向子进程原样恢复。fork 只序列化 fd、模式、回调地址/上下文及实际待写、未读、回退字节，在子进程重建锁和缓冲。宽字符流实现 UTF-8 解码/编码、方向、EOF/EILSEQ 和 ungetwc，复用现有字符转换器。
- libc 的 GNU strerror_r 修正为字符串指针返回值，XSI __xpg_strerror_r 独立返回正错误码。错误号格式化使用栈内存，保留 errno。此处曾使 LLVM 的错误打印将整数 0 当作字符串指针；临时定位代码已删除。
- `exec.rs` 中的 POSIX spawn 移至 `exec/spawn.rs`；`spawn/storage.rs` 管理客体内存中的操作记录、路径字符串和紧凑 PATH 候选。删除发布到 guest 的 Box<Vec<FileAction>>，fork 前释放临时 Rust 对象，子进程从自己的客体副本恢复操作，父子销毁互不影响。
- Debian Clang 已完成真实编译/链接/执行，并通过与 GCC 共用的线程、TLS、插件构造器、80 位数学、stdio 和 fork 探针。包准备补齐经归档验证的四个标准头文件目录别名，保留 LLVM 私有目录；未添加编译器环境变量或替代编译器。
- 新增真实 ELF 探针：`CookieStreamsProbe.c` 覆盖回调、短写重试、宽字符和两代 fork；`SpawnRuntimeProbe.c` 覆盖 chdir/open/dup2/closefrom、CLOEXEC、进程组、信号掩码、PATH、外层 fork 后复用和独立销毁，以及失败后的 errno/PID/子进程回收。共同编译探针增加 GNU/XSI 错误字符串的 ABI 与边界检查。

验证：`artifacts/modular-tools-final.json` 的 31 项检查为 **29 通过、2 失败、无缺输入**。GCC/Clang 编译行为、Node 扩展行为、Python、Bash、压缩、Debian 打包/安装/移除、SQLite、Redis 持久化及新增 stdio/spawn 均通过。`artifacts/modular-java.json` 验证默认 JVM 的 JIT、线程、文件和连续 8 次 ProcessBuilder；`artifacts/modular-sshd-final.json` 完成真实密钥登录、远程管道与退出码 23，测试服务随后回收。FFmpeg 当前首先缺 `XIBarrierReleasePointer`，PostgreSQL 当前首先缺 `towupper_l@GLIBC_2.3`，仍不能记为可用。

`artifacts/modular-stdio-spawn-build.log` 的完整串行构建通过：1715 Rust 测试通过、24 helper 忽略，18 导出与 17 依赖测试通过，smoke 的进程/对象/事务均归零。此前 `clang-strerror-build.log` 在与工具矩阵并行时出现 3 项进程/锁/句柄时序失败；串行完整回归未复现，尚不能据此断定其根因。删除无消费者的 SDK 后，`artifacts/modular-cleanup-build.log` 再次完整通过：1709 Rust 测试通过、24 helper 忽略（减少的 6 项属于已删除 SDK），导出/依赖测试及 smoke 均通过。`artifacts/modular-package-check.json` 确认 36 个原生映像与入口字节未因清理改变，均与构建 DLL 一致；顶层三项以及空 PATH、无关 cwd 下自动定位相邻 rootfs 启动通过。因此上述工具验收仍对应最终二进制；测试类与临时诊断源文件已从发布 rootfs 移除。

边界仍保留：stdio 在 fork 准备时遇到被其他操作持有的流锁会返回 EAGAIN；关闭的 FILE 控制块仍保留至进程结束，缓冲和回调对象已释放。宽字符路径对应现有 UTF-8 编码配置，不能视为完整 locale 支持。尚未测量这些修改对整体 RSS、fork 延迟或 Unix socket 吞吐的影响。

## 2026-09-17 前批：GCC 与 Redis 行为闭环、Clang 依赖与接口

发布目录仍只有 `init.exe`、`worker.exe`、`rootfs/`。`-Development` 仅为已安装编译器的 rootfs 在 `usr/lib/x86_64-linux-gnu/` 发布无版本名 ELF 链接输入；版本化 SONAME 仍指向 `rootfs/lib/` 的原字节 PE 实现。没有 SDK 目录、运行时 ELF 中转或模块清单。Debian 开发包准备完成后执行开发打包，更新 ABI 后也需重新打包；普通构建保留 rootfs 中已有文件。

- GCC 已完成真实 C 编译、链接与执行。扩展探针覆盖 stdio、9 参数 scanf、80 位数学、PE 数学地址验证、ELF 插件构造器、线程局部数据、pthread 和 fork 父子独立状态。`artifacts/compiler-runtime-final.json` 的 `gcc-runtime` 通过。
- Redis 已完成 AF_UNIX 通信、事务/WATCH、Lua、哈希/有序集合、1 MiB 二进制往返、BGSAVE、BGREWRITEAOF、关闭和 AOF 重启恢复。`artifacts/redis-runtime-alias-fixed.json` 包含三项行为标记，最终矩阵再次运行同一行为探针。
- libm 新增精确处理 x87 80 位输入的 lroundl/llroundl、ceill/floorl/truncl；llrint/llrintf 直接别名到同 ABI 实现。真实 ELF 验证四种舍入模式、超过 double 精度的整数/小数、整数边界、无效异常与 errno 保留。`artifacts/extended-integer-probe/report.json` 通过。
- 修复导出生成器复制别名时保留旧 name 的错误，补齐 `_Exit` 与 isoc99 扫描入口。scanf/vscanf 使用现有扫描器和 System V 寄存器/栈游标，没有另起解析实现。线程调度入口直接导出 libpthread 已有实现。
- 新增 pthread_mutex_clocklock，复用 realtime/monotonic 截止时间及 SRW 锁路径；无竞争成功时不读时钟、不验证无需使用的 timespec。回归覆盖两种时钟超时、过去时间、非法时钟与纳秒字段。该路径仍使用已有的有界退避轮询，尚未做定时锁吞吐基准。
- SIGSTOP 的等待报告与 stopped 状态在同一次共享表更新中发布，消除父进程看到 stopped 却读到旧 continued 报告的窗口；不在冻结线程后获取表锁。完整回归及额外五次真实跨进程 stop/continue 均通过。
- 依赖锁新增 9 个 Clang/开发包，总计 274 包。准备检查 2826 文件、435 依赖边；纠正 libclang 的标准目录别名，保留 LLVM 私有目录。核心 LLVM 导入检查通过，完整开发闭包仍缺 `fopencookie`、`catgets`、`fgetwc`、`ptrace`、`__stpcpy`、`fallocate64`；不能将 Clang 计为可用。

验证：`artifacts/clang-clocklock-build.log` 完整构建通过，1714 个无过滤 Rust 测试通过、24 helper 忽略，18 导出和 17 依赖测试通过，smoke 回收计数全部归零。`artifacts/compiler-sshd-final.json` 再次完成密钥登录、远程管道和退出码 23。`artifacts/compiler-package-check.json` 验证 36 个原生映像与构建 DLL 字节一致、顶层三项、空 PATH 与无关 cwd 启动。没有测量这些修改的整体 RSS 或吞吐提升。

最终工具矩阵 `artifacts/compiler-redis-final.json` 为 **25 项通过、4 项失败、无缺输入**。本批剩余明确失败：Clang 的两项编译探针当前首先缺 `fopencookie`，FFmpeg 首先缺 SDL 引用的 `XIBarrierReleasePointer`，PostgreSQL 首先缺 `towupper_l`。Java 默认 ProcessBuilder、Docker 网络/隔离等既有缺口继续保留，长期 goal 仍为 active。

## 2026-09-17 前批：发布目录、执行边界与真实登录

当前发布物为 `artifacts/portable-dist/{init.exe,worker.exe,rootfs/}`。自研 DLL 原字节按 SO 名置于 `rootfs/lib/`；第三方 ELF 依赖保留标准目录与包私有插件目录。发布物没有 SDK、host 子目录、模块清单或报告；链接探针使用 `target/debug/elf-imports/`。入口使用独立的静态 Rust 标准库构建，已在其他 cwd、空 PATH 下验证默认相邻 rootfs 启动。Windows/VC Runtime 依赖仍存在，干净 Windows 部署还没有验收。

`ld-linux-x86-64` 保留 ELF/PE 装载、符号、重定位、TLS、dl 接口、DWARF 与装载状态。指令解码、trampoline、AOT 缓存移入 `guest-engine/src/execution/`，VEH 独立为 `veh.rs`；删除无调用的 host trampoline。runtime 的 `CodeImage/CodeConfig/CodeResult` C ABI 只借用输入和输出缓冲区，跨边界不传 Rust Vec/String。AOT 在 `rootfs/var/cache/kinakaze/aot/` 按需生成；应用前验证原始字节，开始应用后出错直接失败。装载器仅保留三个转换计数，不再保存带对象名的逐地址列表。

SSH 真实测试曾卡在认证前的限额与 poll：

- FSIZE 与 NPROC 状态写入 runtime 进程行，fork 继承、exec 保留；普通文件写入/截断执行 FSIZE、短写及 SIGXFSZ。NPROC 实现零额度的非特权创建禁止；有限非零 UID 配额尚未实现，明确拒绝设置。
- poll 的内部等待对象不再占用 Linux fd，故 sshd 将 NOFILE 降到 1 后仍可认证；内部对象不进入 fork 快照，事件缓冲区在单次 poll 中复用。信号回调在内部对象释放后再交付。
- `run-sshd.py` 用临时密钥运行真实 Windows OpenSSH 客户端，完成登录、远程 id、管道排序与退出码 23。验收服务在测试结束时回收。测试为密钥认证、UsePAM=no；PTY/SFTP/PAM 尚未覆盖。
- seccomp 过滤未实现。清除原来 `seccomp()` 无条件成功的返回值，改为 ENOSYS；不能把该 SSH 登录结果当作生产隔离验收。

扩展 Node 测试发现 fresh fork 中 `environ` COPY 绑定丢失，以及 `setenv` 把模块私有 Vec 暴露给客体的问题。环境管理现位于 `libc/src/process/environment.rs`，字符串与发布数组使用 runtime 客体内存；fork 通过固定状态恢复指针和绑定。`putenv` 保留调用方字符串、`clearenv` 发布空指针、直接 environ 赋值后的修改保持一致；不再把 guest 环境变更写入 Windows 环境。删除重复环境实现与私有空数组泄漏。

已完成验证：

| 验证 | 结果与证据 |
| --- | --- |
| 完整构建与回归 | 1714 Rust 测试通过，24 helper 忽略；18 导出与 17 依赖测试通过；smoke 结束后 process/object/transaction 全为 0。`artifacts/portable-verified-build.log` |
| 格式 | `cargo fmt --all -- --check` 通过。`artifacts/portable-format-final.log` |
| 实际 ELF fork | loader-entry、native-math、资源限额/poll、环境两代 fork 均通过；线程 exec 失败恢复与 RELRO 内 IFUNC 接续通过。`artifacts/process-thread-probe/result.json` |
| SSH | 密钥登录、远程执行、管道及退出码 23 通过。`artifacts/portable-sshd-final.json` |
| Node | VM、zlib、Worker、SharedArrayBuffer/Atomics、异步文件、子进程环境/cwd/退出码、2 MiB Unix socket/TCP 往返通过。`artifacts/portable-node-runtime.json` |
| 扩展工具矩阵 | 21 通过、4 失败、1 缺输入。包含 Python 标准库、Bash、归档、procfs、curl/wget、dpkg 构造安装卸载、SQLite 持久事务；Java 此矩阵只验证启动。`artifacts/portable-expanded-final.json` |
| 发布目录 | 顶层三项、30 个原生映像与对应 DLL 字节一致；其他 cwd/空 PATH 启动通过。`artifacts/portable-package-check.json` |

FFmpeg 已补入锁定的 Debian 程序与 ELF 闭包、sndio 实际兼容别名和已实现函数的版本导出，当前实际启动仍缺 `XIBarrierReleasePointer`，静态审计还有图形、音频、宽字符和扩展精度数学接口。Redis 缺 `llroundl@GLIBC_2.2.5`，PostgreSQL 缺 `towupper_l@GLIBC_2.3`；Clang 输入未准备。Java 默认 ProcessBuilder/posix_spawn 仍是独立未解问题。没有用忽略导入、假成功或换程序替代这些失败。

修复包私有目录后，GCC 已越过 LTO 插件查找并实际执行 `/usr/bin/ld`，当前失败是 Debian 开发链接脚本引用的 `/lib/x86_64-linux-gnu/libc.so.6` 与 `/lib64/ld-linux-x86-64.so.2` 不存在。运行时 PE 模块不能直接当作 GNU ELF 链接输入；下一步需要标准开发库路径上的真实链接元数据，不能复制另一套 glibc 代替本项目运行库。复测为 `artifacts/portable-gcc-final.json`，不能将该编译链接探针计为通过。

依赖准备不再默认生成 root 内清单；仅 `--report` 请求时计算并记录安装文件哈希，使用流式摘要避免整份大文件读入内存。Debian 包私有插件不能一律扁平化；GCC LTO、SASL、图形等恢复包内目录，BLAS/LAPACK 的标准查找名保留真实文件内容。性能优化目前有分配和状态量的代码依据，尚无完整 release 模式 A/B 测量，不宣称极致性能已经达到。

## 推进方式与验收目标

| 工作线 | 具体完成条件 |
| --- | --- |
| 模块边界与工程规范 | 语义、宿主资源、协议与 UI 各有 owner；删除经验证无用的重复实现；构建、格式、依赖/导出审计通过 |
| Shell 与日常工具 | Bash 数组、重定向、管道、替换、信号和作业控制；文本/文件/压缩解压、进程信息的真实输入输出正确 |
| Agent 运行环境 | Java JIT/子进程、Python 标准库/扩展/venv、Node 模块/子进程；Clang/GCC 编译并执行产物；curl/wget、FFmpeg 转换通过 |
| SSH 与包管理 | sshd 密钥/认证/PTY/会话/退出、ssh 端到端连接；dpkg 包构造安装卸载、apt 索引与依赖事务通过 |
| 数据库 | SQLite 持久事务与锁、Redis 网络/Unix socket、PostgreSQL 多进程启动和 SQL/重启恢复；不以版本号成功代替数据库可用 |
| 观察能力 | init 显示当前会话真实状态；Linux procfs、FD/task/maps、内存等逐步与真实所有权保持一致；仅按需采集 |
| CUDA | 明确 Linux CUDA ABI 到宿主 NVIDIA 驱动的桥接；设备枚举、上下文、内存、stream/event、模块与 kernel 执行；错误和释放经过验证 |
| 性能 | 固定产物与参数的 A/B、多轮冷热启动、内存/复制/分配/延迟对照；有收益再引入汇编、JIT 或 AOT，保持通用路径和同语义回归 |

遇到独立阻塞记录准确原因，继续其他可验证工作线。目标不因单批完成而关闭。

## 2026-09-16 第一批

### init 按需 WebUI

- `crates/manager/src/diagnostics.rs` 产生一致的只读逻辑进程快照，不含 token、ticket 或状态内容。
- `crates/host-win/src/diagnostics.rs` 查询 Windows 工作集、私有提交内存和累计 CPU 时间，核对 native PID 与创建时间；进程退出或查询失败显示不可用。
- `apps/init/src/web.rs` 和 `web/` 提供页面与 `/api/state`。默认关闭，只允许 `127.0.0.1`；固定两个请求线程、8 KiB 请求头上限、请求读写超时。没有后台采样或指标缓存，native 查询和 JSON 编码在 manager 锁外执行。
- 可见页面每 2 秒请求一次，隐藏页面取消在途请求并暂停，关闭页面即停止请求。已经进入宿主查询的单次请求允许完成。
- `worker run --web 127.0.0.1:0` 启动并打印自动分配的地址。页面生命周期属于该会话，不是全局跨会话控制台。
- 真实 BusyBox 进程 + HTTP JSON 已验证；Chromium 验证桌面/手机布局、进程行、内存值、手动刷新和无脚本异常。截图在 `artifacts/init-webui.png` 与 `artifacts/init-webui-mobile.png`。

### Unix socket 内存路径

- 共享元数据快照不超过 128 字节时使用栈缓冲；普通 Unix socket 队列元数据为 80 字节，经 `read_with` 解码时不再为临时原始快照分配 Vec。
- 同一快照实现在写事务读取旧值和独占检查时复用；大记录保留堆缓冲。保留逐字原子复制与发布代次复检，回调仅在取得稳定快照后运行一次，允许重入。
- 队列编码直接预留 80 字节，减少常规路径的 Vec 扩容。尚未消除全部 I/O 分配，也未把 metadata 快照优化宣称为 payload 零复制。
- 30 项 Unix socket 回归、8 项共享元数据测试通过，覆盖进程死亡、跨进程发布、并发 bank 复用、栈/堆边界和回调重入。
- 显式微基准见 `artifacts/shared-metadata-cost.log`，本轮为 debug 单次诊断，不能据此宣布生产性能倍数。发布模式、多轮端到端基线仍待完成。

### 工具准备与验收入口

新增 `tests/guest/tool-matrix.json` / `tool-matrix.py`，20 个工具探针逐项运行、超时隔离、输出逐项落盘。报告区分 `startup` 和 `behavior`，以及 `passed`、`failed`、`missing`、`timeout`；记录 worker、runtime 和矩阵哈希。

`prepare-root.py` 增加可重复 `--program`、`--tree`、`--busybox-applet` 参数。没有通过放宽 ELF 校验来接受异常 Python 输入。缺依赖仍在修改目标前失败，保留依赖来源与哈希记录。

准备本轮工具目录：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/tool-root --program bin/bash --program usr/bin/node --program usr/bin/ssh --program usr/bin/dpkg --program usr/bin/dpkg-deb --program usr/bin/apt --busybox-applet gzip --busybox-applet gunzip --offline
python tools/provider-catalog/generate.py --extend-evidence artifacts/tool-root
./tools/build.ps1
python tests/guest/tool-matrix.py --root artifacts/tool-root --timeout 15 --report artifacts/tool-matrix-current.json
```

ABI 证据现可增量合并，保留旧应用的版本需求。libc/libpthread 兼容符号转发到已有同一实现，缺少实现的符号不会生成成功桩。

本轮最终结果：

| 检查 | 结果与证据 |
| --- | --- |
| 完整 `tools/build.ps1` | 93 项 Rust 测试通过、2 个子进程 helper 忽略；18 项 catalog 测试通过；管理 smoke 结束后 process/object/transaction 均为 0；`artifacts/v2-progress-build.log` |
| 依赖工具单元测试 | 10 项通过 |
| Unix socket / 共享元数据 | 分别 30 / 8 项通过；`artifacts/unix-regression.log`、`artifacts/shared-metadata-tests.log` |
| Java 原有完整探针 | `JAVA_JIT_THREADS_FILES_OK`、`JAVA_SPAWN_OK`，退出 0；`artifacts/progress-java.stdout.log` |
| curl HTTP/TLS | 9 项通过；`artifacts/progress-curl-report.json` |
| 20 项工具根矩阵 | 6 通过、5 失败、9 缺输入；`artifacts/tool-matrix-current.json` |
| ELF 导入审计 | 缺口从 43 项降至 25 项；`artifacts/tool-import-audit.json`；审计仍返回失败，不能当作完整覆盖 |

工具根已通过 shell、tar/gzip 往返、procfs、curl/wget 下载行为，以及 SSH 客户端版本启动。该根没有复制 Java；已有 Java 根的启动和完整功能探针另行通过。dpkg-deb 已跨过 `pthread_sigmask` 缺口，下一处为 obstack；apt 已跨过 pthread 缺口，下一处为 `mkstemps`。其余失败和缺输入仍保留在报告中。

### 后续入口

查看 `artifacts/tool-import-audit.json` 与最新工具矩阵。下一批按独立问题推进：

1. Bash 缺 `getservent` 族；补齐服务数据库迭代及相关 ABI，不以空列表规避需求。
2. Node 的 `__libc_stack_end` COPY 对象缺真实定义；关联 guest 启动栈和重定位生命周期处理。
3. dpkg 的 obstack 函数存在旧实现但未形成完整前端 ABI；先审查 callback/布局与边界，再登记和验收实际打包。
   apt 当前下一处缺口是 `mkstemps@GLIBC_2.11`，临时文件创建必须保持独占创建、后缀和失败清理语义。
4. Python 现有本地文件的动态字符串表超出 PT_LOAD 文件范围；获取可追溯的完整 Linux 发行文件与标准库。sshd 缺 `libcrypt.so.1`，补受校验依赖及服务配置。
5. GCC、Clang、FFmpeg、SQLite、Redis、PostgreSQL 的本轮工具根尚无输入，准备闭包和对应功能探针。Java 使用已有 `artifacts/guest-root` 验证。
6. CUDA 尚未实施；按设备/provider 独立模块推进，不能把 OpenGL 成功等同 CUDA 可用。

当前仍未达到完整 Bash/Node/Python、Debian 包管理、数据库或 CUDA 验收。每轮更新实际证据，不用导出数量替代应用行为。


## 2026-09-16 第二批：Bash 与 Debian 包事务

### 已实现

- GNU obstack 从 `misc.rs` 的旧实现独立为 `obstack.rs`。修正 x86-64 布局、带参数 callback 的首次分配、空对象生存期、增长搬迁和完整链释放；登记真实函数与失败处理对象，不再保留两套实现。
- 临时文件统一经 fdio 创建：六个 X 验证、保留后缀、随机重试、0600、独占创建，并保留 CLOEXEC/APPEND 等标志。`mkstemp`、64 位和带 suffix/flags 入口复用一条路径。
- 服务数据库拆到 `netdb/services.rs`。按需读取 guest `/etc/services`，支持 aliases、任意协议名、set/get/end 迭代和 rewind；已有空 guest 文件不会退回宿主服务表。准备脚本按存在情况复制 services。
- Bash 的 `mbrlen` / `__mbrlen` 与 `mbrtowc` 共用已有 UTF-8 状态机，删除旧的无状态宽字符解码。隐式状态各自独立，并纳入 fork 快照；补齐 `wcsdup`、`imaxdiv`。
- `opendir` 和 `scandir` 从已打开的目录 FD 枚举，保留真正的打开错误；修复缺失配置目录被报告成 ENOTDIR 的问题。补齐 `/dev/fd/N` 打开和 stat 到现有 procfs FD 路径的转发，Bash 进程替换可运行。
- DNS wire name skipping 严格检查边界和 label 类型，失败保留 cursor；libresolv 转发到同一 libc 实现。补齐当前无翻译目录 locale profile 的 gettext 单复数入口，未声称支持完整消息目录。
- `__libc_stack_end` 由 loader 在 guest 初始化器前发布实际初始栈指针，通过 COPY redirect 更新可执行文件副本；libc 与 ld-linux frontend 共享一个真实对象。Node 已跨过该 COPY 解析缺口，仍有后续缺口。

ABI 核对依据：[GNU obstack 布局](https://raw.githubusercontent.com/bminor/glibc/release/2.39/master/malloc/obstack.h)、[临时文件语义](https://man7.org/linux/man-pages/man3/mkstemp.3.html)、[服务数据库](https://man7.org/linux/man-pages/man3/getservent.3.html)、[restartable mbrlen](https://man7.org/linux/man-pages/man3/mbrlen.3.html)、[DNS name skipping](https://raw.githubusercontent.com/bminor/glibc/release/2.39/master/resolv/ns_name_skip.c)。

### 可复现准备和验证

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/tool-root --program bin/bash --program usr/bin/node --program usr/bin/ssh --program usr/bin/dpkg --program usr/bin/dpkg-deb --program usr/bin/dpkg-split --program usr/bin/apt --program bin/tar --program sbin/ldconfig --program sbin/start-stop-daemon --tree usr/share/dpkg --busybox-applet gzip --busybox-applet gunzip --busybox-applet rm --busybox-applet diff --offline
python tools/provider-catalog/generate.py --extend-evidence artifacts/tool-root
./tools/build.ps1
python tests/guest/run-libc-tools.py
python tests/guest/tool-matrix.py --root artifacts/tool-root --timeout 25 --report artifacts/tool-matrix-batch2.json
```

`tests/guest/libc_tools_probe.c` 是真实 Linux ELF ABI 回归，覆盖 guest 分配/释放 callback、临时文件 flags 和写读、服务枚举/空文件权威性、分段 UTF-8/状态互通/非法输入、宽字符复制、intmax 除法返回 ABI、DNS 截断/压缩、目录错误和重命名、初始栈发布。该 freestanding probe 用 GLOB_DAT 验证栈值；Node 自身的 COPY 导入验证其解析路径，不能把两者合称为完整 Node 验收。

矩阵新增 `dpkg-deb-roundtrip` 与 `dpkg-install-remove`。测试在临时目录生成无外部依赖的包，校验控制字段与载荷，使用独立 admindir/instdir 实际安装、移除和验证文件消失。未运行联网 APT 更新、依赖升级、维护脚本或系统服务安装。根中缺少 rm/diff/ldconfig/start-stop-daemon 等必要命令时，必须补全真实工具输入，不能通过关闭 dpkg 校验来获得通过。

本批验证：

| 检查 | 结果 / 日志 |
| --- | --- |
| 完整构建、catalog、外层 Rust 与 manager smoke | `artifacts/v2-batch2-final-build.log`；93 Rust 通过、2 helper 忽略；18 catalog 通过 |
| GNU obstack / 临时文件 / 服务数据库 | 1 / 1 / 2 项通过；`obstack-tests.log`、`tempfile-tests.log`、`services-tests.log` |
| UTF-8 状态机及 fork / scandir | 9 / 1 项通过；`uchar-shell-tests.log`、`scandir-regression.log` |
| 真实 libc ABI 探针 | `LIBC_TOOLS_OK`；`artifacts/libc-tools-root/stdout.log` |
| 22 项工具矩阵 | 12 通过、1 失败、9 缺输入；`artifacts/tool-matrix-batch2.json` |
| Debian 包行为 | `.deb` 构造/检查/解包，独立目录安装/移除均通过；`DEB_ROUNDTRIP_OK`、`DPKG_INSTALL_REMOVE_OK` |
| Java 完整探针 | JIT/线程/文件和子进程通过；`artifacts/batch2-java.stdout.log` |
| curl HTTP/TLS | 9 项通过；`artifacts/batch2-curl-report.json` |
| ELF 导入审计 | 工具输入扩展后尚余 12 项；`artifacts/tool-import-batch2.json`；仍失败 |

已通过的矩阵项目包括 shell、Bash 数组/进程替换、压缩往返、procfs、curl/wget、SSH 客户端启动、dpkg/dpkg-deb/APT 启动、两个实际包行为。Java 使用已有 guest-root 单独验证，工具根中的 Java 缺输入状态保持可见。

### 发布模式内存路径测量

`shared_metadata_cost_probe` 仍是显式 ignored 探针，正常运行不采集。改为预热、轮换 API 顺序、7 轮样本和按数据大小设定次数。运行：

```powershell
cargo test --manifest-path engine/Cargo.toml -p kinakaze-vfs --release --lib mount::shared::tests::shared_metadata_cost_probe -- --ignored --nocapture --exact
```

日志 `artifacts/shared-metadata-release-cost.log`：同一发布二进制中，80 字节热缓存快照的 `read()` 中位数 44.8 ns，`read_with()` 17.7 ns，每样本 20,000 次。常见 Unix socket 的 80 字节元数据路径省去临时 Vec；这不是跨版本 A/B 或端到端吞吐结果。129 字节处分别为 40.5 / 50.8 ns，4 KiB 为 95.9 / 105.4 ns，大记录没有普遍收益，不扩大栈阈值或宣传全场景加速。本次同宿主还有工具验证任务，严谨性能定额仍需独占环境重复测量。

### 下一批入口

1. Node 当前停在 `__timezone` COPY；剩余导入包括 timezone/tzname 及 GNU aliases、`_environ`、backtrace 系列和 pkey 系列。时区对象必须跟随真实 TZ/rules 更新，不导出恒零对象；pkey 要明确宿主实际能力与失败语义。
2. APT 后续增加隔离的本地仓库索引与依赖事务。dpkg 已完成无维护脚本包的安装/移除，后续覆盖配置文件、触发器、脚本、失败回滚与升级。
3. Python 源文件完整性、sshd 的 libcrypt 依赖及配置、编译器/FFmpeg/数据库输入仍待补全。不要降低 ELF 校验或把工具根缺输入归类为 runtime 成功。
4. 宿主实测 NVIDIA GeForce RTX 4060 Laptop GPU，驱动 596.49、显存 8188 MiB；具备 CUDA 桥接硬件验证条件。当前尚未实现 libcuda provider，也没有 kernel 成功证据。
5. Unix socket 性能继续补发布模式的端到端 payload/延迟/分配基线，再决定汇编、JIT/AOT 和更大结构调整。仍遵守不用就不采集。

长期 goal 保持 active；本批成果不代表 Node/Python/sshd、完整 APT/数据库或 CUDA 已完成。


## 2026-09-16 第三批：真实 CUDA Driver API 计算

上一轮为有实质进展：Bash、包构造/安装/移除和运行时 ABI 已获得新证据。本轮沿 CUDA 独立工作线推进，没有改变长期目标范围。

### 实现与边界

新增 `engine/providers/libcuda`，按 API 声明、宿主装载、动态查询、VFS 文件加载、fork 生命周期分模块；109 个编译导出，其中 106 个转发声明与 3 个查询/VFS 入口。新增模块 ID 29 的 `libcuda.so.1` / `kinakaze_libcuda.dll`，由 catalog 与 frontend 同步工具接入共享 runtime。frontend 生成器现在自动排序 implementation anchors，新增 provider 后可直接通过 rustfmt 检查。

- `nvcuda.dll` 仅在 CUDA API 第一次使用时从系统目录加载；无背景采集、设备轮询或 provider 工作线程。普通转发复用缓存地址，没有桥接层每次调用的堆分配或句柄映射表。
- 设备指针、句柄和参数按核对后的 x86-64 签名交给宿主驱动，实际 GPU 执行任务。没有 CPU 软件计算替代路径，也没有把宿主函数指针直接暴露给 Linux ELF。
- `cuGetProcAddress` 两种 ABI 依 NVIDIA 版本和 stream 模式选择 SysV thunk；通过内部 host API 地址与已审核版本 anchor 比对，避免把旧 32 位分配 ABI、新 context ABI 或未实现接口误配到已有函数。
- `cuModuleLoad` 从 guest VFS 读取 PTX/cubin，预留已知文件大小，处理缺文件/空映像，然后使用原生内存映像加载接口。源码和 cubin 是 GPU 代码，宿主路径不会替代 guest namespace。
- CUDA 初始化前 fork 的子进程可以独立初始化。初始化后的 fork 子进程不能使用父进程 native GPU 状态，相关操作返回 NOT_SUPPORTED；通过 exec 后可重新初始化和计算。CPU fork 本身保持可用。

接口签名由 `tools/cuda/verify-abi.py` 对照校验和锁定的 NVIDIA 12.8 头文件检查，包括 64 位 size_t/设备指针、参数数量、const/pointer 层级、ABI 版本、PTDS/PTSZ 和两个查询入口。`prepare-sdk.py` 将官方 NVIDIA wheel 缓存到 artifacts，校验下载和头文件 SHA-256，不安装进系统 Python、不修改系统 CUDA。正常构建和 PTX 路径不依赖 SDK。

### 真实硬件验证

硬件：NVIDIA GeForce RTX 4060 Laptop GPU，驱动 596.49。CUDA Driver API 实测版本 13020，`cuDeviceTotalMem_v2` 返回 8,585,216,000 字节。

`tests/guest/cuda_probe.c` 是以 Linux x86-64 为目标的 freestanding ELF。由 V2 worker 加载 `libcuda.so.1` facade，验证：

1. 真实设备、显存和 current context。
2. pinned host memory、device allocations、异步双向复制和 GPU memory fill。
3. 直接调用和动态查询的 kernel launch，各自重置输出后核对 1025 个元素，包括不完整的最后一个 block；两条路径均需要实际产生正确结果。
4. nonblocking stream、events、等待、有效 GPU 时间值、模块/stream/event/显存/host memory 释放，以及销毁后的 current context 为 null。
5. 未知接口、宿主存在但未桥接的 API、旧不兼容 ABI、非法 flags 和超前版本的失败行为。
6. fork 前初始化、fork 后 GPU 状态拒用，以及 child exec 后重新建立 context 并完成同一 GPU 计算。
7. 同一 PTX 既由真实 driver JIT 执行，也通过 NVIDIA ptxas 为 sm_89 编成 cubin 后加载执行。

| 检查 | 结果与证据 |
| --- | --- |
| 109 个 CUDA ABI 声明与 anchor | `python tools/cuda/verify-abi.py` 通过 |
| 未使用时不装载驱动 | `artifacts/cuda-unit-tests.log`，1 项 provider 单元测试通过 |
| PTX / fork / exec | `artifacts/cuda-root/fork.report.json`；`CUDA_FORK_BEFORE_INIT_OK`、`CUDA_FORK_GUARD_OK`、`CUDA_EXEC_AFTER_FORK_OK` |
| AOT cubin GPU 执行 | `artifacts/cuda-root/cubin.report.json`；`CUDA_KERNEL_STREAM_EVENT_CLEANUP_OK` 与 `CUDA_DRIVER_OK` |
| 完整构建与 manager smoke | `artifacts/v2-cuda-final-build.log`；93 Rust 通过、2 helper 忽略，18 catalog 通过；退出后 processes/objects/transactions 均为 0 |
| Bash / Debian 安装移除 / curl 回归 | `artifacts/tool-matrix-cuda-regression.json`，3 项行为通过 |

GPU 报告记录 worker/runtime/ELF/源码/PTX 哈希，AOT 报告还记录 ptxas、cubin 哈希与目标架构。硬件不存在或 marker/行为不满足都会失败，不会将缺 GPU 计为通过。

### 后续工作保持完整

CUDA Driver API 的上述计算路径已经有实际证据，完整 Linux libcudart、PyTorch、cuBLAS/cuDNN/NCCL、回调、graphs、arrays/textures、外部内存/IPC 和新结构体仍未验收。当前是进程 ABI 到宿主驱动的桥接，不是向虚拟机分配 PCI 设备。

Node 时区/回溯/pkey 缺口、Python/sshd/编译器/FFmpeg/数据库输入、APT 本地仓库与依赖事务仍按第二批入口继续。内存与 Unix socket 的端到端发布模式基线继续保留；没有据 CUDA kernel 成功宣布全部 JIT/AOT 或性能目标完成。长期 goal 仍 active。

## 第四批：时区数据 ABI、可验证软件包输入与 SQLite（2026-09-16）

### 已完成的行为

- `time/zone_globals.rs` 单独维护 `timezone`（Linux 64 位 long）、`daylight` 和 `tzname[2]`。双下划线别名绑定同一份数据，`_environ` 也绑定既有环境变量存储。
- `tzset`、`localtime_r` 和 `mktime` 共享发布逻辑：标准时区偏移使用 UTC 以西秒数，即使当前在夏令时也不误用夏令时偏移；同时发布标准和夏令时名称。
- 新的 `CopiedValue<T>` 使用 `UnsafeCell` 保存导出负载，独立保存 COPY 重定位目标。64 字节 fork 记录恢复数据和重定位目标；名称位于可继承的 guest 内存，旧 `tm_zone` 在更改 TZ 和 fork 后仍可读。注册时不读时钟、环境或设备，没有后台采集。
- 根目录准备工具新增 `--package`，可选择已锁定的程序、标准库树和相关包。显式选定的 SONAME 优先，包依赖环和不完整 ELF 闭包会在安装根目录前报错。
- 增加 Debian bookworm 的 SQLite 3.40.1-2+deb12u2、Python 3.11.2-6+deb12u8、libcrypt1 1:4.4.33-2 输入。所有包有来源 URL、源包版本、归档大小和 SHA-256；显式文件另有 SHA-256，树内容受归档哈希保护，缓存和安装文件也逐文件校验/记录。
- Debian 绝对符号链接仅在 guest 归档成员表内解析，落地为普通文件。Python 跨包 sysconfig 别名明确由拥有实际文件的 minimal 包提供。下载、解包均不执行维护脚本。
- SQLite 实际完成内存查询，以及磁盘 WAL、事务提交/回滚、进程退出后重新打开、唯一约束失败和 `integrity_check`。软件包测试加入默认 `tools/build.ps1` 检查。

### 验证结果

| 检查 | 结果与证据 |
| --- | --- |
| 时区已有单元测试 | `artifacts/time-unit-tests.log`：27 通过 |
| Linux GOT 与真正 COPY 重定位 | `python tests/guest/run-time-globals.py`：两种 ELF 均输出 `TIME_GLOBALS_OK`，包含 fork 后立即读全局数据、继承的 `tm_zone`、子进程修改和父进程隔离；重定位表保存在 `artifacts/time-globals-root/` |
| 完整构建 | `artifacts/v2-common-final-build.log`：93 Rust 通过、2 helper 忽略；18 catalog、14 guest-deps 通过；manager smoke 清理后对象数为 0 |
| 离线输入校验 | 7 个锁定 Debian 包通过；`tool-root` 准备 694 个文件、279 条 ELF 依赖边 |
| 合并工具矩阵 | `artifacts/tool-matrix-common-final.json`：23 项中 14 通过、3 失败、6 缺输入；报告 runtime SHA-256 与最终 dist 一致 |
| libc 回归 | `artifacts/libc-tools-common-regression.log`：`LIBC_TOOLS_OK` |
| CUDA PTX / fork / exec 回归 | `artifacts/cuda-root/fork.report.json`：passed，runtime SHA-256 与最终 dist 一致，1025 元素 GPU 计算正确 |
| Java 独立根目录回归 | `artifacts/java-common-regression.stdout.log`：`JAVA_JIT_THREADS_FILES_OK`、`JAVA_SPAWN_OK`，退出 0 |

工具矩阵的通过项是 shell、Bash、归档、procfs、curl、wget、SSH 版本、dpkg 版本、dpkg-deb 版本、Debian 打包/解包、包安装/移除、APT 版本、SQLite 查询和 SQLite 磁盘事务。版本检查仍标为 startup，并未视为完整服务或网络事务验收。

### 下一轮入口

1. Node 已越过时区导入，当前停止在 `backtrace@GLIBC_2.2.5`。需要真实 guest ELF 回溯；已有代码没有通用 DWARF unwinder，不能用 Windows PE 回溯或空成功结果冒充。随后仍有 pkey 相关缺口。
2. 新下载 Python 的 ELF/标准库输入和依赖闭包有效，当前停止在 `preadv64v2@GLIBC_2.26`。现有 `preadv64/pwritev64` 通过移动文件位置实现，补 v2 时应同时解决位置保持、短读写、flags 和不可寻址 FD 的语义。
3. sshd 加入了真实 libcrypt 依赖，当前停止在 PAM 的 `setfsuid@GLIBC_2.2.5`；完整认证、用户身份切换和服务会话仍需验证。
4. GCC、Clang、FFmpeg、Redis、PostgreSQL 尚缺此测试根目录的输入；Java 已在独立根目录验证，不重复复制大型 JRE 到工具矩阵根目录。
5. IANA zoneinfo 文件读取仍是时区模块的后续工作。新增数据导出沿用现有 POSIX TZ/Windows zone 规则，没有宣称支持完整地理时区数据库。

`README.md` 和 `tools/guest-deps/README.md` 保留可复现命令。按需信息采集策略不变。长期 goal 保持 active。

## 第五批：定位 I/O、真实 Python 运行与 HTTPS（2026-09-16）

### 已完成的行为

- 将 libc 定位向量 I/O 移到 `fdio/positioned.rs`，新增 `preadv64v2/pwritev64v2` 及对应别名，统一长度溢出检查、短读写和部分成功返回。普通 `readv/writev` 复用相同的长度/错误处理，不分配合并所有向量的数据缓冲区。
- 修复 `pwrite64/preadv64/pwritev64` 的 seek/restore 实现。原生文件通过 `kinakaze-vfs::positional::File` 在整次定位操作期间持有 inode pin，同步句柄需要独立 reopen，显式偏移不会改动原生游标或共享 FD 位置。管道带非负定位偏移返回 ESPIPE，不再悄悄读写流数据；v2 的 offset=-1 保留普通流语义。
- tmpfs 使用已有显式位置接口；保留 Linux O_APPEND 行为、零长度向量和 null/zero/full/random 设备语义。临时原生句柄在信号回调、fork/exec 前释放。
- `__sysconf` 绑定既有实现；libc 的 `copysign` 直接转发到同一 libm 实现。没有复制实现或用空函数解决导入。
- 移除启动层写死的 `PYTHONHOME=/usr/python3/python`。Debian Python 现在根据实际位置找到 `/usr/lib/python3.11`，已有显式 guest 环境覆盖入口不变。
- 根目录同时提供 OpenSSL 默认 CA 路径 `/usr/lib/ssl/cert.pem`。修复前 Python 的默认 SSL context 信任库实测为 0 个证书；修复后探针要求 x509_ca 大于 0，并验证真实 HTTPS 及错误证书/主机名拒绝。
- 小型 ELF 回归共用 `tests/guest/elf_probe.py`，记录运行时、worker、源码和 ELF 哈希以及超时/退出状态。工具矩阵可引用本地测试源码并记录哈希；仅在实际选中的 HTTP 探针需要时创建 HTTP fixture，不为其他测试启动空闲服务线程。

### 实际验证

| 检查 | 结果与证据 |
| --- | --- |
| VFS 同步句柄与 FD 复用 | `artifacts/positional-unit.log`：原生游标不变，关闭并替换 FD 后，既有操作仍读写原文件 |
| libc 文件 I/O 模块 | `artifacts/fdio-python-regression.log`：66 通过、1 个显式 fixture helper 忽略 |
| 真实 ELF 定位 I/O | `artifacts/positioned-io-root/report.json`：passed；原生文件、实际挂载 tmpfs、dup 共享位置、O_APPEND、短读、后续向量错误前的部分传输、溢出、管道、设备及不支持 flags |
| Python 基础与 agent 标准库 | `artifacts/tool-matrix-python-final.json`：`python`、`python-runtime` 均通过 |
| Python 深入行为 | Unix socket/selectors、线程池、asyncio TCP 回显与子进程、preadv/pwritev、SQLite 事务、bz2/lzma/zlib、ZIP/TAR、Unicode 环境与 cwd、Python exec、SSL CA/MemoryBIO |
| 实际本地 HTTP/TLS | `artifacts/python-curl-tls-report.json`：原有 9 项 curl 检查与新增 Python HTTPS 检查全部通过；Python 收到正确二进制响应，并拒绝错误 CA 和错误主机名 |
| 完整构建 | `artifacts/v2-python-prefix-build.log`：93 Rust 通过、2 helper 忽略，18 catalog、14 guest-deps 通过；manager smoke 退出后清理正常 |
| 合并矩阵 | 24 项中 16 通过、2 失败、6 缺输入；root 为 695 文件、279 条 ELF 依赖边；报告 runtime 哈希与最终 dist 一致 |
| 其他回归 | `LIBC_TOOLS_OK`；时区 GOT/COPY/fork 两种 ELF 通过；CUDA PTX/fork/exec 真实 GPU 计算通过；Java `JAVA_JIT_THREADS_FILES_OK` 和 `JAVA_SPAWN_OK` 通过 |

复现 Python 与网络检查：

```powershell
python tests/guest/run-positioned-io.py
python tests/guest/tool-matrix.py --root artifacts/tool-root --only python --only python-runtime
python tests/guest/curl-smoke.py --worker target/debug/kinakaze-worker.exe --root artifacts/tool-root --dist dist --python --report artifacts/python-curl-tls-report.json
```

### 未完成范围与后续入口

- v2 定位 I/O 当前支持 flags=0；RWF_NOWAIT、DSYNC/SYNC、APPEND/NOAPPEND、ATOMIC 等非零 flags 明确返回 EOPNOTSUPP，不会假装完成这些保证。这里的 O_APPEND 指 FD 的既有状态。
- 向量仍通过分段原生请求处理。跨独立 open-description 并发写入的整组原子性、流向量调用期间的完整 FD pin，以及 tmpfs 多向量事务需要继续统一，不能据当前测试宣布完整 readv/writev 并发语义已经验收。
- Python 已覆盖上述 agent 常用标准库行为，未据此宣称所有扩展、pip 包和 ML 框架完成。软件包生态、任意第三方 native 扩展继续按实际运行验证。
- 矩阵剩余失败仍是 Node 的 guest ELF 回溯入口 `backtrace` 和 sshd/PAM 的 `setfsuid`。编译器、FFmpeg、Redis/PostgreSQL 输入与行为验证、APT 仓库事务、Unix socket/内存发布模式性能基线继续推进；Java 在独立根目录验证。

本批没有根据代码形式宣称端到端性能提升，也没有新增常驻指标采集。完整长期 goal 保持 active。


## 第六批：目标扩展、直接 PE .so 与数学模块拆分（2026-09-16）

### 更新的执行目标

新增要求统一记录在 [goal.md](goal.md)：移除中转 DLL、独立 ld.so、ELF/PE 混合装载、缩小 runtime、明确 fork/exec 生命周期、减少硬编码与环境变量、完整 Docker 能力和按需采集。现有活动 goal 保持执行，当前工具只能更改其完成/阻塞状态，不能改写活动目标文本，因此没有通过虚假完成重建 goal。

“拆除 ld.so”按移除旧依赖空壳、建立有清晰职责的独立混合装载模块执行。当前 ld-linux facade 仍存在，不能将这一批宣称为完整 ld.so 拆分。此前 sshd 的缺口调查暂存在 `artifacts/sshd-imports.json`：sshd/libpam 仍缺 setfsuid/setfsgid 等 11 个符号；尚未以占位函数填充。

### 实际改动

- manifest v2 必须声明 `pe` 或 `elf-facade`。`ModuleImage` 分别持有真实 PE 映像与待迁移 facade；没有装载失败后换格式的路径，也不根据文件扩展名冒充 ELF。
- `libs/libm` 直接链接数学实现。构建产物 DLL 原样以 `dist/host/libm.so.6` 发布，函数地址直接交给 guest linker；没有对应的 ELF 跳板或中转 DLL。dladdr 使用实际 PE 地址范围，PE 不伪造 ELF program headers。
- libc 的历史数学入口通过具名原生导入库调用独立 libm。导入名使用实现前缀，避免把 Windows CRT 的同名数学调用截获成 SysV ABI。Rust 的 raw-dylib 不支持此处的 SysV ABI，因此使用标准 MSVC import library；调用约定仍由 Rust extern 声明确定。
- libm 不静态依赖 guest TLS、分配器、VFS 或进程状态，仅通过原生 C ABI 导入 canonical errno 指针。runtime 不再链接数学实现；其数学实现导出从 135 个降为 0。
- fork 对 PE 只登记原生模块生命周期；只有 facade 的额外映射进入 guest mapping 登记。保留既有 SDK 绑定恢复，已验证真实 fork 子进程继续调用原地址且访问正确 errno。
- 打包器拒绝原生 PE guest 导出再经过转发。清理严格依据上一版验证后的清单，发布后删除其不再拥有的文件；旧的 libm 转发 DLL 和 ELF facade 已从正式 dist 移除。v1 清单仅在打包升级时显式迁移，worker 不兼容猜测旧格式。
- 移除 `KINAKAZE_LIB_EXE` 构建环境变量，使用正常 MSVC 工具链 PATH。原生单元测试依赖独立模块，构建脚本在测试调用期间限定 PATH 至本次 dist/host，并在 finally 恢复；不新增 guest 搜索环境变量。
- 新增共享 ELF 探针的显式库参数、额外依赖哈希及编译器错误输出；SDK fork fixture 适配 manifest v2。

### 验证与边界

- `artifacts/native-math-full-build3.log`：完整构建、18 项 catalog 测试、14 项 guest-deps 测试、91 项外层 Rust 测试通过，2 个子进程 helper 按原设计忽略；管理 smoke 退出后 processes/objects/transactions 均为 0。
- `artifacts/native-math-unit.log`：5 项数学实现测试通过，覆盖 x87/80 位数、浮点环境、复数 ABI、CRT 递归边界及范围错误。
- `python tests/guest/run-native-math.py`：真实 Linux ELF 输出 `NATIVE_MATH_OK`，检查 dladdr 的 MZ 映像、直接函数地址、GNU 版本正反例、负零、80 位 ABI、线程 errno 隔离、fork 后 math/errno 和父进程状态。报告同时记录 libm、runtime、worker、源码及 ELF 哈希。
- `artifacts/native-math-sdk-fork.log`：真实 guest 的嵌套 SDK Copy/Share/Reset 与重绑定输出 `PROVIDER_FORK_OK`。
- `artifacts/tool-matrix-native-math.json`：仍为 16 passed / 2 failed / 6 missing，Python 综合、SQLite 事务、Bash 和 dpkg 行为均通过。Node 仍缺 backtrace，sshd 仍缺 setfsuid；没有扩大成功范围。
- Java JIT/线程/文件/ProcessBuilder、CUDA Driver/PTX/fork/exec、时区 GOT/COPY/fork 均再次通过。curl 和 Python HTTPS 的 10 项正反验证通过；分别见 `native-math-java.*.log`、`native-math-cuda.log`、`native-math-time.log`、`native-math-tls.json`。
- `artifacts/native-math-layout.json`：debug runtime 文件从 24,502,272 字节变为 24,316,416 字节，独立数学 PE 为 800,256 字节。这只说明本次二进制布局；没有将拆分宣称为总内存或整体速度提升，也没有并发回归时测性能。

### 下一步入口

1. 将 provider 装载/登记与 fork 恢复从 runtime 大文件继续拆开，收敛原生模块的初始化、引用、卸载和状态恢复接口，再形成独立 ld.so；当前其余 28 个模块仍包含旧结构。
2. 对 libm 保留的 SDK 管理状态接口继续减负。共享 errno 仍通过明确的 runtime 原生依赖连接；后续迁移 libc/TLS 时一起收敛接口，不能复制状态或静默装回旧库。
3. 继续清理具名 runtime 查找及仅作中转的环境变量；迁移时先确保 fork/exec、Python/Java 和 CUDA 回归通过。
4. 完整 Docker 纳入后续验收。已阅读旧项目 `docs/docker-stats-latency.md` 与现有工具入口，保留其“无订阅不采集、不能篡改应用一秒统计语义”的边界；旧项目结果不算 V2 Docker 验收。使用隔离数据目录验证真实 dockerd/containerd/runc 的生命周期、namespace/cgroup/mount/网络能力。
5. 缺失程序/ABI 继续推进，避免一直停留在一个兼容点。完整目标尚未达到。


## 第七批：装载职责、无状态模块与 Docker 实际入口（2026-09-16）

### 装载和生命周期

- 新增 `crates/loader`，从 runtime 的 guest 入口抽出 provider 装载、符号登记、原生映像引用和 fork 重绑定。它不拥有 RPC 连接或管理会话；runtime 通过明确的 API resolver 提供完成子进程接续后的新管理表。目前仍是单次静态链接的 rlib，独立物理 ld.so 尚未完成。
- manifest 升为 v3，格式与生命周期分别声明。`none` 模块不注入管理表；`runtime-api-v1` 必须提供本模块内的四个可执行 SDK 导出。打包器在发布前拒绝缺失、转发、数据地址或与无状态声明矛盾的管理导出。旧 v1/v2 只在打包清理清单时显式迁移，运行时不尝试旧格式。
- libm 去掉 ABI/SDK 依赖、RPC 状态接口和无用的 fork 重绑定记录。数学实现、实际函数地址、GNU 版本、共享 errno 与 x87 行为保留；跨模块管理 smoke 改用确实拥有管理状态的 libpthread。
- fork 记录只包含模块基址和映像范围，不再序列化初始化函数地址。子进程从已恢复模块重新解析初始化入口，验证范围并调用；恢复后的登记可继续支持嵌套 fork。大小查询和快照写入直接使用协调器缓冲区，不再额外分配完整 payload。
- `LoadedModule::pin` 获取独立 Windows loader 引用，不按文件名重查、重载或分配路径缓冲。模块卸载与 fork 登记撤销在同一事务中完成，避免并发 fork 观察到已卸载映像。测试验证空/内部地址拒绝、原 owner 释放后继续持有以及最终卸载。

### 配置清理与验证

- 移除 `KINAKAZE_V2_WEBUI` 环境变量开关。只通过 `--web` 明确启用；探针不再复制宿主环境来删除该变量。没有新常驻采集。
- `run` 要求显式 `--root`、`--dist` 及绝对 Linux 程序路径。移除 CLI 从环境读取目录、默认 `dist` 和猜测 `/usr/bin`、`/bin` 的路径。内部 fork/exec 仍使用既有宿主启动记录，相关环境传递尚未全部替换。
- `artifacts/loader-final-build.log`：完整构建，92 项外层 Rust 测试通过、2 个 helper 忽略，18 项 catalog 与 14 项依赖测试通过；管理 smoke 退出后对象、事务和进程均清零。
- `artifacts/loader-layout.json`：debug libm 从 800,256 字节降至 345,600 字节（约 56.8%），SDK 管理导出为零。当前 runtime 为 24,350,208 字节；这仅是二进制文件布局，不是 RSS 或速度测量。
- 真实 ELF math/errno/版本/fork、SDK 嵌套 Copy/Share/Reset、时区 GOT/COPY/fork、Java JIT/线程/ProcessBuilder、CUDA PTX/fork/exec 和 curl/Python HTTPS 正反例均在本批通过。对应 `loader-math.log`、`loader-sdk-fork.log`、`loader-time.log`、`loader-java.*.log`、`loader-cuda.log`、`loader-tls.json`。
- 完整工具矩阵 `artifacts/tool-matrix-loader.json` 仍为 16 passed / 2 failed / 6 missing。最终 CLI 清理后再次验证 native math、Python 综合、Bash、dpkg 安装移除；见 `loader-final-math.log` 和 `loader-cli-tools.json`。
- `artifacts/loader-cli-report.json`：5 项实际检查通过，覆盖显式参数、旧环境变量不启用 WebUI、`--web` 返回真实进程指标。状态快照见 `loader-web-snapshot.json`。

### Docker 验收入口

- 从旧项目只读取程序输入，独立准备 `artifacts/docker-root`。46 个文件、88 条 ELF 依赖边均已解析；没有使用旧项目的运行结果充当 V2 成功证据。
- 新增 `tests/guest/docker-matrix.json`，沿用通用有界 probe runner。六个核心程序 Docker CLI、dockerd、containerd、shim、ctr、runc 的实际启动检查，以及 runc 生成非空 OCI 配置，均已通过。
- `DockerDaemonProbe.sh` 在一个管理域内启动真正的 dockerd，使用本次创建的独立 data/exec/socket 目录，通过真实 Unix socket 请求 Docker API，并在退出时停止 daemon、清理其临时目录。不关闭 iptables、桥接或切换 storage driver 来绕过失败。
- dockerd 已进入网络控制器初始化，当前失败于 iptables 子进程。独立执行定位为 `libxtables.so.12` 导入 `getnetbyname@GLIBC_2.2.5` 未实现；不是 Docker API 已通过。
- `artifacts/docker-matrix-loader.json`：7 passed / 3 failed，失败为 daemon API 与 iptables/ip6tables 启动。
- `artifacts/docker-imports.json` 审计实际根目录，当前 5 个缺失符号为 `getnetbyname`、`getnetbyaddr`、`getprotobynumber`、`ether_aton`、`ether_ntoa`。后续先实现真实的网络数据库及地址解析，继续运行默认 daemon；不能用空函数填充导入。

复现 Docker 输入和行为：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/docker-root --program usr/bin/docker --program usr/bin/dockerd --program usr/bin/docker-proxy --program usr/bin/containerd --program usr/bin/containerd-shim-runc-v2 --program usr/bin/ctr --program usr/bin/runc --program usr/sbin/iptables --program usr/sbin/ip6tables --offline
python tests/guest/tool-matrix.py --root artifacts/docker-root --manifest tests/guest/docker-matrix.json --report artifacts/docker-matrix-loader.json --timeout 45
```

下一批继续推进上述 5 个实际 Docker ABI 缺口、无硬编码网络数据库、剩余 provider 去中转及独立 ld.so 的版本化边界。完整 Docker 的容器创建/exec/网络/卷/资源生命周期、Node 回溯、sshd 身份与认证、完整包管理和其他工具仍未完成。长期 goal 保持 active。


## 第八批：按需网络数据库、默认 Docker API 与 inode 清理（2026-09-16）

### 实际改动

- 网络数据库拆为 `netdb/records.rs`、`protocols.rs`、`networks.rs` 和 `ether.rs`。共用逐行读取与记录所有权，查找只保留当前行并只为命中记录构造返回存储；不全量载入配置，不设后台缓存。打开的客体 FD 在调用结束释放。
- 补齐 `getnetbyname/getnetbyaddr/getprotobynumber/ether_aton/ether_ntoa`，同时补齐对应地址转换和 `_r` MAC 接口。网络名称/别名、host-order 数值、历史八/十六进制地址、协议名称大小写、线程存储和 fork 继承指针均有真实 ELF 验证。
- 删除旧 `getprotobyname` 将未知协议一律当 TCP 的实现。协议与网络查找只读取客体 `/etc/protocols`、`/etc/networks`；缺文件、空文件和查找失败明确返回，不查询宿主或内置列表。这里仅实现文件后端，不等于完整 NSS；既有 services 的宿主/常量回退仍待清理。
- 为 xtables 插件补 `index/rindex` 到既有 `strchr/strrchr` 实现的 ABI 别名。目录当前为 29 模块、4421 guest 导出、1211 runtime 内部绑定；数量不是兼容性证明。
- 软件包校验支持精确锁定的 `amd64` 和架构无关 `all`，拒绝与 control 不符及其他架构。增加 netbase 6.4 与 iptables 1.8.9-2 的官方包/源包信息、大小及 SHA-256，显式文件另有哈希。iptables 的完整插件树受归档哈希校验，归档内链接落地为真实 ELF 字节。
- 旧来源的部分 xtables 别名是零字节文件加 NTFS 专有元数据，普通复制后不是 ELF。本批换用明确锁定的 Debian 输入，没有新增猜测旧格式或失效后换实现的路径。当前 Docker root 为 177 文件、323 条 ELF 依赖边；工具 root 加入 netbase 后为 700 文件、279 条边。
- 修复 VFS 删除只读 inode 的失败。统一通过 `FileDispositionInfoEx` 的 POSIX delete 与 `IGNORE_READONLY_ATTRIBUTE` 删除名称，移除 legacy delete 回退和仅用于 socket 的误导命名。已有 FD 继续访问原 inode，原只读属性不被修改；挂载写限制和 native DELETE 权限检查仍保留。
- 生产数学导入库的 link 声明移到非测试 extern 块，避免原生测试的数学实现 rlib 与生产 import library 重复符号；运行时仍使用同一独立 libm，没有替代实现。

### 验证结果

| 检查 | 结果与证据 |
| --- | --- |
| 完整构建 | `artifacts/network-docker-final-build.log`：92 外层 Rust 通过，2 helper 忽略；18 catalog、15 guest-deps 通过；管理 smoke 清理后 processes/objects/transactions 为 0 |
| 网络解析单元测试 | `artifacts/network-db-unit-final.log`：11 通过；含 ABI 布局、别名、非 UTF-8 字节、数值和 MAC 边界 |
| 真实网络数据库 ELF | `artifacts/network-db-guest-final.log`：`NETWORK_DB_OK`；文件替换/缺失/空文件、线程、fork 旧指针、200 次查找后 FD 回收均通过 |
| inode 回归 | `artifacts/readonly-inode-regression.log`：11 通过；新增只读文件删除后 FD 可读、属性不变及原名称可重新创建 |
| Docker 导入审计 | `artifacts/docker-imports-final.json`：147 个 ELF、2100 个版本需求，无缺失导入；运行时路径仍需行为验证 |
| Docker 基础矩阵 | `artifacts/docker-network-final.json`：10 passed，包含六个核心程序、runc spec、iptables/ip6tables 和默认 daemon 的 Unix socket API |
| Docker API 与清理 | 返回 ServerVersion 29.7.2、`DOCKER_DAEMON_API_OK`；正常停止 daemon，完整移除包含 BuildKit 只读文件的临时目录，退出 0；探针扩展后 `artifacts/docker-api-final.json` 再次通过 |
| 常用工具矩阵 | `artifacts/tool-matrix-network-final.json`：16 passed / 2 failed / 6 missing；Node 与 sshd 仍失败，缺少输入不算通过 |
| 其他回归 | `network-db-libc-tools.log` 的 `LIBC_TOOLS_OK`、`network-db-io.log` 的 `POSITIONED_IO_OK`、`network-db-sdk-fork.log` 的 `PROVIDER_FORK_OK`；`network-db-math.log` 的 `NATIVE_MATH_OK` |

当前 Docker 准备方式替代第七批的旧插件输入：

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/docker-root --package netbase --package iptables --program usr/bin/docker --program usr/bin/dockerd --program usr/bin/docker-proxy --program usr/bin/containerd --program usr/bin/containerd-shim-runc-v2 --program usr/bin/ctr --program usr/bin/runc --offline
python tests/guest/run-network-db.py
python tests/guest/tool-matrix.py --root artifacts/docker-root --manifest tests/guest/docker-matrix.json --only docker-daemon-api --report artifacts/docker-api.json --timeout 150
```

### 后续边界

默认 daemon 验收没有禁用 iptables/bridge 或指定替代 storage driver。真实镜像导入、容器创建/exec/重启、网络通信、卷和资源隔离仍需分别验证。新增 `docker-container-lifecycle` 探针覆盖本地 BusyBox ELF 镜像导入、默认桥接分配、PID/exec、绑定目录、退出状态和重启。首次实际运行 `artifacts/docker-container-first.json` 明确失败：进入 containerd 解包后，临时挂载卸载返回 EBUSY，随后临时目录清理也失败；尚未到容器创建。基础 10 项通过不能当作完整 Docker 能力完成。

下一项先从 `mount::unmount_in` 的挂载父子关系检查与解包线程的 mount namespace 路径核查该 EBUSY，不直接改成成功或强制 lazy detach。当前代码的这一检查针对子挂载，不应未经证据写成打开文件仍占用。

Docker 多进程日志存在内容覆盖，启动期间也有 xtables 等待；共享文件位置跨 fork/exec 是待核查方向，尚未确认原因。当前 debug 启动用时不能作为发布模式性能结论。没有新增后台指标采集，长期 goal 保持 active；物理 ld.so、其余 provider 去中转、Node/sshd 和其他工具仍继续推进。


## 第九批：共享文件位置与 mount 传播修复（2026-09-16）

### 已定位并修复

- `ofd::with_once` 在共享位置事务结束时错误地读取第一个同 description 的 FD。读/写/seek 实际只更新调用 FD，stdout/stderr 等 dup 别名会把旧位置重新发布，覆盖此前日志。现在直接读取完成操作的原 FD，并校验 generation，避免发布复用槽位的状态；本地事务也增加相同的 generation 校验。
- 独立原生测试在修复前稳定失败（写入后位置应为 2、实为 1），修复后通过。覆盖别名读、写、seek、共享存储内位置、关闭原 FD 后继续使用别名和最终 inode 回收，见 `artifacts/ofd-alias-before.log`、`ofd-alias-after.log`。
- 新增 `ForkIoProbe.sh` 与工具矩阵的 `fork-shared-file` 项，实际运行 shell/BusyBox fork/exec、继承 FD 3/4、stdout/stderr 合并和精确内容对比。`artifacts/fork-io-tools.json` 中该项和 Python 综合检查通过。
- Docker 失败快照显示新建临时共享 bind 的下面出现相同路径片段的重复子挂载。mount 传播将本次刚创建、继承相同 peer group 的 bind 纳入接收者，因此把创建事件传播回自身，随后非 lazy 卸载正确地因残留子挂载返回 EBUSY。
- 传播模块现在在发布任何新增 attachment 前确定每个 peer group 的接收者，排除本次新增/移动 attachment；保留向已有 peer/slave 的传播。没有放宽卸载检查，也没有改用强制或 lazy detach。
- 新增带真实共享 namespace、子目录 bind 和已有 peer 的回归：修复前出现 1 个不应存在的子挂载，修复后为 0，正常卸载同时撤销 peer 的对应挂载。`artifacts/mount-self-event-before.log` 为失败证据；`mount-propagation-after.log` 的 16 项 topology 测试全部通过，包括 peer/slave、递归 bind、隐藏挂载、ID 不复用与生命周期。
- 临时 runtime 诊断已移除。Docker 测试仅在失败时读取一次 `/proc/self/mountinfo`；诊断读取自身失败不会跳过原有停止 daemon 与清理流程。不增加运行时环境变量或后台采集。

### 最终运行结果与下一项

- `artifacts/mount-ofd-final-build.log`：完整构建通过，92 项外层 Rust、18 项 catalog、15 项 guest-deps 通过，2 个既有 helper 忽略；管理 smoke 的进程、对象和事务退出后均清零。
- `artifacts/docker-mount-final.json`：默认 daemon API 和实际本地镜像导入通过，输出 `DOCKER_DAEMON_API_OK`、`DOCKER_IMAGE_IMPORT_OK`；containerd 的临时 bind 不再出现重复子挂载，解包/卸载完成。日志按顺序保留，之前观察到的覆盖现象在本轮未再出现。
- 容器生命周期探针仍明确 failed（退出 125）：推进到 runc create 后，在 jail rootfs 时 `mount dst=., flags=MS_REC|MS_SLAVE` 返回 EINVAL。尚未运行容器内的 exec/卷/PID/重启断言；不能将这些编写好的断言视为通过。失败后 daemon 停止、临时数据目录完整移除。
- `artifacts/tool-matrix-mount-final.json`：25 项中 17 passed / 2 failed / 6 missing，增加的通过项为真实 fork 共享文件；Node 和 sshd 缺口保留。
- `artifacts/mount-ofd-java.json`：Java JIT/线程/文件与 8 次默认 ProcessBuilder 派生通过。`mount-ofd-positioned.log` 为 `POSITIONED_IO_OK`，`mount-ofd-sdk-fork.log` 为 `PROVIDER_FORK_OK`。最终 Docker、工具矩阵和 Java 报告 runtime 哈希均与本次 dist 一致。

下一项从 runc 的 pivot/chroot、保留 cwd 和相对挂载坐标继续，验证旧根目录上的递归 slave 转换，不放宽 mount 错误条件。上述修复不代表完整并发 FD 关闭语义或完整 mount namespace/容器能力已经验收。没有根据 debug 运行或代码结构宣称端到端性能提升；长期 goal 保持 active。


## 第十批：namespace 根目录模块与 manager 共享对象隔离（2026-09-16）

### 根目录与 fork/exec

- 将不可变 namespace 基准目录、可变 process root、overlay root 和状态编码拆到 `path/root.rs`。V2 在 guest 启动前从明确的 `GuestConfig.root` 初始化基准，不能继续用 worker.exe 所在目录解释客体的旧根 FD。
- 删除生产路径中的 `KINAKAZE_GUEST_ROOT` 选择和错误时返回 `.` 的路径。独立原生调用仍保留 executable-directory 初始化，尚未统一为显式启动记录；没有宣称全项目已经无隐式路径或无环境变量。
- 根目录交接帧升级为 `CYPATH02`，分别保存原始基准、当前根和 overlay 状态；原生路径保持 UTF-16，拒绝相对路径、NUL、长度溢出、未知标志、截断、尾随数据及基准切换。完整解析后才安装状态。编码不再先构造临时 UTF-16 Vec。
- `pivot_root_probe.c` 重现 runc 的 stacked pivot：打开新旧根、pivot_root(".", ".")、fchdir 旧根、递归 slave、detach、切换新根并读文件；在 fork 子进程与父进程分别执行。修复前保留旧根的 fchdir 返回 EIO，见 `artifacts/pivot-root-before.log`；最终 `pivot-root-domain-final.log` 输出两次 `(unreachable)/` 和 `PIVOT_ROOT_OK`。

### 完整实例标识

- 并行原生路径测试曾读取到另一个 hosted Docker 的挂载。定位为共享对象使用固定 root PID namespace 编号 1 作为“实例”标识；PID 表自身虽然已按 manager epoch 隔离，mount 等对象没有。
- `authority::domain_id` 提供完整 64 位 manager epoch，首次需要时读取并缓存。已安装 authority 时身份错误不会切换到 standalone 域；standalone 原生调用使用明确的 0 域。fork 保持原实例，exec 由新 worker 的 authority 重新取得同一标识。
- 更新原先错误依赖 root PID namespace 的挂载、策略、cgroup、时间、IPC、tmpfs/mqueue、终端、用户网络和 Unix socket 共享对象。挂载交接帧升为 `CYMNS004`，原 4 字节域加保留字改为完整 8 字节域；删除无调用的旧 namespace ID 包装，命名不再混用这两个概念。
- Unix abstract 地址的底层 pipe 名另有遗漏：只隔离挂载/共享元数据后双实例测试仍因同名 socket 失败。补齐地址管道、inode 管道、待接受连接及 queue lock 的实例前缀。原有长地址哈希编码仍存在，不能宣称所有命名/回退已清理。
- 新增真实双 manager ELF 测试：第一个保持挂载及 abstract socket 存活，第二个必须看不到其挂载，并能绑定相同 abstract 名称。`domain-isolation-before.log` 记录旧构建的挂载泄漏；`domain-isolation-after.log` 记录中间构建的 socket 泄漏；`domain-isolation-final.log` 和 `artifacts/domain-isolation/report.json` 最终通过。另有高 32 位不同而低位相同的共享对象测试，确保未截断 epoch。
- 这些测试覆盖上述共享对象改动及真实 mount/socket 行为；不等于所有持久化文件、权限边界和资源类型都已完成多实例隔离审计。

### 验证与后续缺口

| 检查 | 结果与证据 |
| --- | --- |
| 完整构建 | `artifacts/domain-root-final-build.log`：92 外层 Rust、18 catalog、15 guest-deps 通过，2 既有 helper 忽略；管理 smoke 退出后进程、对象、事务清零 |
| 原生路径/共享对象/路由 | `domain-root-concurrent-path.log`：17 通过；`domain-shared-tests.log`：9 通过、1 显式性能测试忽略；`domain-route-tests.log`：2 通过 |
| 真实状态恢复 | `pivot-root-domain-final.log` 的 `PIVOT_ROOT_OK`；`domain-sdk-fork.log` 的 `PROVIDER_FORK_OK` |
| 工具矩阵 | `artifacts/tool-matrix-domain-root.json`：17 passed / 2 failed / 6 missing；Node 与 sshd 仍失败，缺输入不算通过 |
| Java | `artifacts/domain-java.json`：JIT、线程、UTF-8 文件及 8 次默认 ProcessBuilder 派生通过 |
| CUDA | `artifacts/domain-cuda.log`：真实 RTX 4060 上 PTX 计算、流/事件清理、fork 前初始化与 fork 后上下文限制通过；不等于完整 CUDA runtime 生态 |
| Docker | `artifacts/docker-domain-root.json` 仍 failed；已输出 API、镜像导入、容器启动三项明确 marker |

Docker 的新启动 marker 只在 Running=true、默认桥接 IP 非空、容器 PID 1/proc 检查和卷内容经宿主客体进程读回后输出；增加有界 readiness 等待，避免立即 exec 读取尚未创建的卷文件。它验证 IP 分配，不是容器网络通信验收。

当前 `docker exec` 在 runc nsexec-1 创建 stage-2 时返回 EAGAIN；尚未确认具体 fork 阶段，不能直接认定为缺内存。容器退出状态、重启和正常删除断言未达到。失败后 dockerd 强制关闭，任务目录仍 EBUSY、netns 目录非空；探针如实失败，未改成忽略资源清理。supervisor 结束后本轮进程树已退出，残留目录保留为调查证据。

本轮临时开启既有 fork timing 诊断，仅收集到外层进程，不能据此判断 runc 内部失败点；没有新增默认采集、后台线程或生产环境变量。实例 ID 缓存及减少临时编码分配尚未形成端到端性能测量。后续继续 clone/setns 生命周期与清理、物理 ld.so/其余 provider 去中转，以及 Node/sshd/编译器/数据库缺口；长期 goal 保持 active。

## 第十一批：统一网络存储、setns 根目录事务与 exec 收养关系（2026-09-16）

### 移除重复网络存储

- 根网络 namespace 原来将 route/sysctl 与 nftables 分别写到客体根目录下的持久化状态文件，嵌套 namespace 则使用共享对象。两个 manager 使用同一根目录时会读到对方的状态，旧文件还可能携带上一实例的 veth/namespace 编号。真实双实例探针在旧构建中报 `manager network state mismatch`，见 `artifacts/network-domain-before.log`。
- `route_state.rs` 和 `nftables.rs` 统一使用既有 namespace Store；删除根目录哈希锁、两套文件映射/校验和/槽位读写、FlushViewOfFile 以及损坏状态返回默认 ruleset 的路径。根和嵌套 namespace 使用同一事务协议，生命周期由对象引用决定，旧持久化文件不再参与运行。
- 存储继续按需提交页，遵守 Store 容量检查；没有以无界分配代替原限制。根网络初始化编码失败直接返回错误。netlink 发布事件也按完整 manager domain 标识命名。
- nftables 过期 generation 或 mutation 失败时，不发布新规则或递增外层网络 generation。单元测试验证拒绝过期请求、失败修改回滚和整个网络快照不变。
- 真实双 manager 测试覆盖同一根目录与不同根目录；同名 abstract socket、挂载和 ip_forward 均隔离。同域 fork/exec 继承网络状态，唯一持有者直接 exec 后仍保留状态；测试不靠另一个进程替它保活。

### mount setns 与 fork

- 失败诊断已定位之前的 runc EAGAIN：子进程进入 HandoffRestore 后，VFS section 3 的 cwd 恢复失败；不是已证明的内存不足。新增 participant/section/stage 上下文只在既有显式 fork 诊断开关下记录，临时强制采集已撤回，默认不写诊断文件。
- mount setns 不能只更换挂载表。Linux `mntns_install` 会将 root 和 cwd 都设为目标 namespace 的根，见 [Linux namespace.c](https://raw.githubusercontent.com/torvalds/linux/master/fs/namespace.c)。实现拆到 `mount/namespace.rs`，核查私有 fs_struct 和权限，先在当前任务中准备目标拓扑及目录引用，全部成功后发布进程 namespace 归属；失败恢复旧拓扑和完整 fs 状态。
- `shared::enter` 仍供内部 fork 状态恢复使用；fork 必须保留保存的 root/cwd，不能套用外部 setns 的重置语义。发布前取得 CURRENT 锁，避免锁错误发生在进程归属已变更之后。
- 新增真实 ELF 探针：一个子进程 pivot 并 detach 旧根，父进程从根外 cwd 进入它的 mount namespace，验证 `/`、文件访问、立即 fork，以及回到原 namespace。修复前稳定失败于 cwd 未重置，`setns-root-before.log` 保留红测；修复后为 `SETNS_ROOT_FORK_OK`。
- 新增根目录解析失败的回滚测试，核对进程归属、任务 namespace、root/cwd、目录对象引用、overlay、confined 和 umask。已有传播测试的实际文件改放到 namespace 根内，并以稳定客体路径引用不存在的挂载子路径，避免依赖 standalone 宿主路径访问。

### 保留 exec 时的实时父进程

- 将 Docker 等待问题缩小为独立 ELF 探针：subreaper 收养另一个 PID namespace 的 init，以及通过 setns/fork 创建的进程；被收养者随后 exec 并退出。旧实现稳定返回 `waitpid(5)=-1 errno=10`，同时 `/proc/5/status` 的 PPid 为 0；红测见 `network-setns-pidns-reaper-before.log` 和 `network-setns-pidns-reaper-detail.log`。
- 原因是 VFS 首次注册强行使用 manager 启动身份里的 parent，并调用可覆盖现有 row 的 register。它覆盖了共享 PID 表刚完成的 subreaper 收养关系，还会清掉 exec 后应保留的进程标志。新 `claim` 操作在 PID 表锁内核查并采用已有 row，仅为全新 row 使用初始值；不再以锁外快照覆盖实时状态。显式更新仍使用 register，职责分开。
- `getppid` 从维护 Linux 收养关系的共享 PID 表读取，不再重复查询 manager 的启动父进程。manager 仍负责 PID 分配和 native 进程事务，没有移除或绕过其身份校验。收养语义核对 [Linux exit.c](https://raw.githubusercontent.com/torvalds/linux/master/kernel/exit.c) 的 `find_new_reaper`。
- 新原生测试使用真实存活/退出进程验证 claim 不覆盖收养后的 parent/group/session/flags。ELF 测试进一步验证收养后 exec、精确退出码 37、外层收养者在子 PID namespace 内不可见，以及 subreaper 标志 fork 不继承、exec 保留；最终输出 `PIDNS_REAPER_EXEC_OK`。

### 验证与边界

| 检查 | 结果与证据 |
| --- | --- |
| 完整构建 | `artifacts/network-setns-reaper-final-build.log`：92 外层 Rust、18 catalog、15 guest-deps 通过；2 既有 helper 忽略；管理 smoke 退出后进程、对象、事务清零 |
| 进程生命周期 | `artifacts/reaper-runtime-unit.log`：28 通过；`reaper-vfs-job-unit.log`：17 通过；`network-setns-pidns-reaper-final.log` 的 `PIDNS_REAPER_EXEC_OK` |
| 挂载单元测试 | `artifacts/setns-mount-unit-final.log`：127 通过、4 忽略，包含 setns 回滚和传播；忽略项包含既有性能探针及祖先 atime 缺口 |
| 网络单元测试 | `network-route-unit-final.log` 2 通过，`network-nft-unit-final.log` 2 通过，`network-multicast-unit-final.log` 5 通过 |
| ELF 根目录恢复 | `setns-root-final.log` 的 `SETNS_ROOT_FORK_OK`，`network-setns-pivot-final.log` 的 `PIVOT_ROOT_OK` |
| 真实实例隔离 | `network-domain-final.log`、`network-domain-distinct-final.log`；报告位于 `artifacts/domain-isolation-shared/report.json` 和 `artifacts/domain-isolation/report.json` |
| 常用工具 | `artifacts/tool-matrix-network-setns-reaper.json`：17 passed / 2 failed / 6 missing；Node、sshd 失败保留，缺输入不算通过 |
| 其他实际程序 | `network-setns-sdk-fork.log` 的 `PROVIDER_FORK_OK`；`network-setns-java.json` 的 JIT/线程/文件和 8 次派生通过；`network-setns-cuda.log` 的真实 GPU PTX/fork/exec 通过 |
| Docker 生命周期 | `artifacts/docker-network-setns-reaper.json`：passed、退出 0，最终输出 `DOCKER_CONTAINER_LIFECYCLE_OK`；运行约 127 秒，仅为本次 debug 验证耗时 |

中间构建 `docker-network-setns-final.json` 到达 exec 载荷后仍超时。探针将容器 PID 1 的退出请求移到 exec 返回之后，避免同时退出造成歧义；此顺序仍能复现问题。修复实时收养关系后，最终构建实际通过 exec 的 PID/绑定卷断言及命令返回、容器退出码 23、inspect 状态、再次启动并退出 23、删除容器和镜像、`docker ps -aq` 为空；随后正常关闭 daemon 并完整删除临时目录。没有启用替代网络/storage driver，没有忽略 rm 失败。最终进程树已退出，成功运行目录 `kinakaze-docker.1mnvxf` 已不存在；此前失败目录保留为证据。

最终 runtime SHA-256 为 `cbbf775128ad5980cdff0e1a88372bf233e7e9403fa8792100797a90e01a280a`，Docker、工具矩阵、Java、CUDA 与最新 ELF 报告记录对应哈希。SDK fork 使用单独生成的测试 manifest 和 distribution。

日志仍有 runc cleanup 返回 255、任务目录 rename 权限错误、cgroup `memory.events` watch 缺失、bridge sysfs 属性、AF_PACKET/conntrack 告警。容器重启和显式删除虽通过，不能据此宣称 daemon 存活期间所有内部资源均及时回收，也不能把桥接 IP 分配当作容器网络通信通过。

本批删除了重复网络文件映射及写回路径，没有测得端到端性能或 RSS 收益，不作性能提升承诺。默认后台采集未增加。下一项验证容器实际网络通信与 daemon 存活时的资源回收，收敛上述明确告警；继续 services 回退清理、物理 ld.so 与 provider 去中转、Node/sshd 和缺少输入的工具。长期 goal 保持 active。

## 第十二批：客体 services、流式枚举与真实网卡 ioctl（2026-09-16）

### 服务数据库与生命周期

- 删除 `/etc/services` 查询失败后读取 Windows 服务表以及内置常见服务清单的路径。客体文件缺失、空文件、查询未命中分别按接口契约返回；`getaddrinfo` 先从客体文件解析命名服务，再将数字端口交给地址解析器，宿主不再决定服务名。`getnameinfo` 未命中时按其标准契约输出数字端口。
- `netdb/services` 拆成记录解析与返回存储、枚举游标、调用者缓冲 ABI、地址解析四个职责。与 protocols/networks 共用客体文件 Reader，移除重复文件读取代码；保留原始字节名称、别名、大小写和任意协议标签。
- 不缓存整个 services 文件。读取缓冲为 8 KiB，另加当前行和匹配记录的内存；查询结束即关闭自己的 FD，不改变枚举位置。枚举仅在使用时打开，`endservent` 关闭。没有后台加载、扫描线程或定时刷新。
- 枚举首次使用时才登记 fork participant。准备阶段锁住游标，交接只复制 FD、未消费缓冲及 EOF；子进程在 VFS FD 恢复后接管，保留共享文件偏移和各自的用户态缓冲。恢复帧校验长度、标识、FD 和 EOF 状态。原生测试验证拒绝损坏帧，真实 ELF 验证返回指针继承、父子各自消费缓冲、EOF 后追加仍需 rewind，以及 rewind 清除 EOF。
- 替换总是 ENOENT 的 `getservbyport_r` 占位实现，并增加 `getservbyname_r`。调用者缓冲中存放所有字符串和对齐的别名指针表；不足时先返回 ERANGE，既不发布半个 servent，也不改写数据缓冲。真实 ELF 检查未对齐缓冲、别名终止、大小写、未命中和结果指针。
- files 后端的枚举和独立查询行为核对 [glibc files-XXX.c](https://raw.githubusercontent.com/bminor/glibc/master/nss/nss_files/files-XXX.c)；名称/协议与端口处理核对 [files-service.c](https://raw.githubusercontent.com/bminor/glibc/master/nss/nss_files/files-service.c)。这不是完整 NSS 实现；原地址解析器的 AI_ADDRCONFIG 重试等其他旧行为尚未清完。

### 去除网卡 ioctl 的固定数据

- 独立网络 namespace 的 ELF 探针首先在 IPv4 回环失败；发现旧 libc `SIOCGIFFLAGS` 固定报告 UP，`SIOCSIFFLAGS` 直接返回 0，没有改变网络状态。旧查询还返回固定的 eth0、192.168.1.100、MAC、MTU 和索引，未知接口也得到这些数据。
- 从终端模块删除这段实现，移到 VFS `usernet/ioctl.rs`。查询使用已有真实网络拓扑和地址；支持 flags、IPv4 ifconf/地址/目的地址/广播/掩码、MTU、硬件地址和索引，未找到接口返回 ENODEV。读取的是创建 socket 的 namespace，unshare 后旧 socket 仍访问原 namespace。
- 设置 flags 检查目标 namespace 的 CAP_NET_ADMIN，使用拓扑锁和网络事务；宿主接口的实际修改明确返回 EOPNOTSUPP，已经满足的标志请求可成功。loopback 启停及标准地址创建提取为共同逻辑，ioctl 与 netlink 使用同一路径并保留通知。
- 接口标志与权限边界核对 [Linux dev_ioctl.c](https://raw.githubusercontent.com/torvalds/linux/master/net/core/dev_ioctl.c) 和 [dev.c](https://raw.githubusercontent.com/torvalds/linux/master/net/core/dev.c)。当前入口覆盖 INET socket，尚未证明所有 socket family、地址别名和所有网卡写操作兼容。

### 验证与边界

| 检查 | 结果与证据 |
| --- | --- |
| 完整构建 | `artifacts/services-ioctl-build.log`：92 外层 Rust、18 catalog、15 guest-deps 通过，2 既有 helper 忽略；管理 smoke 退出后进程、对象、事务清零 |
| 数据库单元测试 | `artifacts/services-ioctl-unit.log`：12 通过 |
| netlink 回归 | `artifacts/services-ioctl-netlink-unit.log`：21 通过，包含 multicast 和原有 Docker bridge 请求 |
| 真实数据库 ELF | `services-ioctl-guest.log` 的 SERVICES_DB_OK、`services-ioctl-network-db.log` 的 NETWORK_DB_OK、`services-ioctl-libc-tools.log` 的 LIBC_TOOLS_OK |
| 真实网络 namespace | `artifacts/inet-namespace-after.log`：接口启停、实际 IPv4 地址枚举、未知接口、旧 socket 的 namespace 归属、IPv4 和 IPv6 各自回环载荷通过 |
| 双栈缺口 | `artifacts/inet-dualstack-before.log`：IPv4、IPv6 单独回环通过，IPv6 通配监听接受 IPv4 连接仍返回 ECONNREFUSED；独立保留失败探针 |
| 常用工具 | `artifacts/tool-matrix-services-ioctl.json`：17 passed / 2 failed / 6 missing；Node、sshd 失败及缺输入继续如实保留 |
| HTTP / TLS | `artifacts/curl-services-ioctl.json`：实际 curl 二进制 GET/POST、localhost 解析、跳转、404、证书信任与主机名拒绝，以及 Python HTTPS 验证通过 |
| Docker 生命周期复测 | `artifacts/docker-services-ioctl.json` 的 docker-container-lifecycle：passed、退出 0，约 124 秒；exec、退出码 23、再次启动、删除及 daemon 清理通过 |
| Docker IPv4 网络 | 同报告的 docker-container-ipv4-network：容器内 HTTP 内容与回环载荷通过；第二容器连接服务 IP 172.18.0.2 返回 EIO，未达到 TCP 和 DNS 成功 marker，整项保持 failed |

中间构建的 Docker 默认监听网络探针 `artifacts/docker-network-services.json` 通过 API、镜像导入并确认服务容器运行，但容器内 IPv4 wget 被拒绝，未达到跨容器通信；失败后的 overlay/netns 清理也未通过。诊断发现 `/proc/net/tcp` 和 tcp6 仍返回固定的端口 80 记录，因此这些输出不作监听状态证据，最终探针已移除这段误导性诊断。该旧 procfs 占位数据需要下一批清理并接入真实状态。

最终 Docker 探针分开保留默认监听和显式 IPv4 监听，后者用于独立验收 IPv4 链路，未替换或隐藏双栈失败。生命周期成功目录 `kinakaze-docker.AvUDog` 已删除；IPv4 网络失败目录 `kinakaze-docker.faiLVf` 的 overlay/netns 残留保留为调查证据，测试进程树已退出。下一步缩小跨 namespace 地址选择/可达性返回 EIO 的位置，补齐同桥容器 TCP、DNS 和失败路径清理。

最终 runtime SHA-256 为 `6a894b153c02cc2974953f789b9828b90b6925a32636bf523d08c46d62fbdbdd`。本批没有新的默认环境变量或后台采集，也未测得端到端性能/RSS 收益，不能将流式读取和删除固定数据等同已测量的性能提升。物理 ld.so、其余 provider 去中转和完整 Docker 仍未完成，长期 goal 保持 active。

## 第十三批：真实路由/ARP 视图与桥接范围验证（2026-09-16）

### 删除固定网络信息

- 删除 `/proc/net/route` 内固定 eth0、192.168.1.x 网关和固定 loopback 路由，删除 `/proc/net/arp` 内固定 IP/MAC。新 ELF 在旧构建中稳定失败于“空 namespace 不应包含固定或宿主路由”，红测见 `artifacts/procnet-route-before.log`。
- 文本格式放到 `procfs/network.rs`；只在读取相应 proc 文件时生成。客体路由复用 netlink 的显式路由和地址派生路由，筛选 IPv4 主路由表，不把 local 表条目混进来。网关、掩码、优先级、reject/host 标志以及 MSS/window/RTT 指标按记录生成；旧 RefCnt/Use 按 Linux 接口固定为 0。多路径及 nexthop ID 尚未实现格式化，明确返回 EOPNOTSUPP。
- `hostnet/routing.rs` 独立负责通过 Windows SDK 读取宿主 IPv4 路由和邻居表，使用 SDK 结构偏移及 RAII 释放系统表。仅初始网络 namespace 读取宿主表，其他 namespace 不泄露宿主路由或 ARP。
- ARP 输出实际邻居状态和硬件地址；未解析条目按接口地址长度显示零地址。排除 loopback、IFF_NOARP、组播和广播的静态映射。当前虚拟链路使用 IP transport，没有实现 L2 邻居缓存，因此其 ARP 表为空；这不是完整 ARP 协议支持。
- 没有后台采样、持久缓存或网络探测流量；接口、路由和邻居查询都由实际读取触发。没有测量端到端性能或 RSS 提升。
- 字段与主表边界核对 [Linux fib_trie.c](https://raw.githubusercontent.com/torvalds/linux/master/net/ipv4/fib_trie.c)，邻居输出核对 [arp.c](https://raw.githubusercontent.com/torvalds/linux/master/net/ipv4/arp.c)。宿主观察使用 [GetIpForwardTable2](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getipforwardtable2) 和 [GetIpNetTable2](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getipnettable2)，不修改 Windows 配置。

### 路由和桥接验证

- 新的真实 ELF 进一步暴露 RTM_NEWROUTE 对已有 loopback 返回 ENODEV：旧存在性检查仅查看普通虚拟网卡列表。修复为同时承认该 namespace 的实际 loopback 索引，保留未知接口拒绝路径。
- 最终 ELF 验证空 namespace 的 route/ARP 表、接口启用、通过 netlink 新增指定前缀与 metric、从 procfs 读回，再删除并确认消失；输出 PROCNET_ROUTE_OK。已有 IPv4、IPv6 单独回环及 socket namespace ioctl 探针继续通过。
- 新增原生最小测试：三个私有 namespace 中配置同一桥及两对 veth，两侧实际 TCP 交换载荷；随后启动独立原生进程作为客户端，通过同一共享拓扑完成连接和载荷传递。测试中普通 socket 后端通过，不启用替代 packet 后端。它证明直接及跨原生进程的基本桥接路径，不等同 Docker/runc 的 fork、exec、setns 流程全部通过。
- Docker IPv4 探针继续复现第二容器连接服务 IP 的 EIO；容器内 HTTP 通过，未达到跨容器 TCP 和 DNS marker。显式诊断没有定位该错误；临时错误追踪和测试环境转发已撤回，没有增加默认诊断或生产环境变量。后续需用真实 ELF 缩小 Docker 创建/恢复路径与最小测试的差异。

### 验证记录

| 检查 | 结果与证据 |
| --- | --- |
| 最终完整构建 | `artifacts/procnet-route-final-build.log`：92 外层 Rust、18 catalog、15 guest-deps 通过，2 既有 helper 忽略；管理 smoke 退出后进程、对象、事务清零 |
| proc 格式与边界 | `artifacts/procnet-route-unit.log`：2 通过；检查字节序、标志、缩放指标、损坏属性拒绝和 local 表排除 |
| 实际宿主表 | `artifacts/hostnet-live-tables-final.log`：1 通过；接口、地址、路由、邻居都来自当前系统，验证 ARP 不包含 loopback/广播映射 |
| netlink 回归 | `artifacts/procnet-route-netlink-unit.log`：21 通过 |
| 桥接与跨进程 | `artifacts/bridge-direct-process.log`：1 主测试通过、1 helper 忽略；主测试实际调用 helper 并验证退出与载荷 |
| 最终 ELF | `artifacts/procnet-route-final.log` 的 PROCNET_ROUTE_OK；`procnet-route-inet.log` 的 IPv4/IPv6/namespace ioctl 成功 marker |
| 宿主 proc 读取 | `artifacts/procnet-root-route.log`、`procnet-root-arp-final.log`，实际 BusyBox 读取动态路由和 ARP；路由日志来自本批中间构建 |
| 最终工具矩阵 | `artifacts/tool-matrix-procnet-route-final.json`：17 passed / 2 failed / 6 missing；Node、sshd 和缺输入继续保留 |
| 中间构建 Docker | `artifacts/docker-procnet-route.json`：生命周期 passed（约 126 秒），IPv4 网络 failed（容器内 HTTP 通过，跨容器 EIO，约 163 秒） |

最终 runtime SHA-256 为 `98b0fb36c118ff2e457533f75d46819a3e1e70021e9bbaf29a8c9eb9aad1453d`。Docker 报告对应此前中间构建 `c4c702c0c7b5bec77cb4147183de68069f91554f48fbec0a71da13ad363df984`，不能把它算成最终二进制的 Docker 全量复测；其后修复了 loopback 路由检查及 ARP 过滤，最终 ELF 和对应单元测试覆盖这些改动。生命周期成功目录 `kinakaze-docker.hKfZzg` 已删除，网络失败目录 `kinakaze-docker.CpLWlB` 残留保留，测试进程树已退出。

`/proc/net/tcp`、tcp6、udp、udp6 仍有旧占位数据，本批没有伪称完成所有网络 procfs；双栈、跨容器 EIO、DNS、内部资源回收、物理 ld.so 和其余转发模块等仍在队列。下一批兼顾装载器/provider 模块化与真实 ELF 网络路径，避免只重复完整 Docker 启动。长期 goal 保持 active。

## 第十四批：装载器入口归一、职责拆分与 RTLD_NEXT 修复（2026-09-16）

### 删除重复实现与无用状态

- 真实 ELF 通过显式 libdl 句柄取得 dlsym，再调用 RTLD_NEXT；旧构建稳定失败于“libdl RTLD_NEXT lost the guest caller”，见 `artifacts/loader-entry-before.log`。原因是 libc/libdl 的 Rust 转发函数先创建自己的栈帧，装载器记录到转发函数的返回地址，而不是客体调用位置。
- 删除 `engine/providers/libdl` 项目和 runtime 依赖，同时从 libc 的 misc 模块删除另一套 DLL 名称查找、CRT 初始化、函数指针数组、空指针失败返回及转发函数。未增加替代环境变量或运行时符号探测。
- catalog 从实际 guest-engine rlib 读取 8 个装载器入口，供 libc/libdl 两套公开 ABI 使用；缺少已编译目标或存在重复 libc 实现时，生成阶段失败。libc 同时补齐 dlvsym 导出，版本仍只采用已有 ELF 观察证据。现在是 4,423 个客体导出、1,205 个 runtime 内部导出。
- 从 guest-engine 大文件拆出 `host/loader/mod.rs`（唯一 live linker、递归锁、fork 发布和线程错误）、`lookup.rs`（装载、查询与引用释放）、`info.rs`（地址与程序头）及 `dlinfo.rs`（GNU 查询）。装载器状态仍只链接一次，原有 fork 锁定和子进程恢复顺序保持；不是复制内核状态的第二个 DLL。
- libdl 声明 `lifecycle=none`，删除 shim-sdk 依赖和初始化入口，不再登记无用的 SDK 模块或 fork API 绑定。PE 导出检查只见 8 个直接指向 canonical runtime 入口的转发记录，没有管理导出；见 `artifacts/loader-entry-pe-exports.log`。
- 调用地址语义参考 [glibc dlsym.c](https://raw.githubusercontent.com/bminor/glibc/master/dlfcn/dlsym.c) 与 [dlvsym.c](https://raw.githubusercontent.com/bminor/glibc/master/dlfcn/dlvsym.c)：入口向实际查找传递调用者返回地址。本批保留已有 naked SysV 入口，删除遮住调用者的额外 Rust 包装。

### 验证与边界

| 检查 | 结果与证据 |
| --- | --- |
| 最终完整构建 | `artifacts/loader-entry-final-build.log`：92 外层 Rust、19 catalog、15 guest-deps 通过；2 既有 helper 忽略；管理 smoke 退出后进程、对象、事务清零 |
| 装载器单元测试 | `artifacts/loader-entry-unit.log`：10 通过，包括移动后的 dlinfo 缓冲与启动契约测试 |
| 真实 ELF 装载入口 | `artifacts/loader-entry-final.log`：LOADER_ENTRY_OK；显式 libc/libdl 句柄、普通/版本化 RTLD_NEXT、未知版本拒绝、共享且只消费一次的 dlerror、父子 fork 后查询与关闭全部通过 |
| 独立 PE 模块 | `artifacts/loader-entry-native-math-final.log`：NATIVE_MATH_OK；native PE 地址、版本、线程 errno、fork 后地址和导入恢复通过 |
| Linux Java | `artifacts/loader-entry-java.json`：实际 JVM JIT、线程、UTF-8 文件与 8 次默认 ProcessBuilder 派生通过，约 8 秒；不是性能对比结论 |
| 常用工具 | `artifacts/tool-matrix-loader-entry-final.json`：同一 tool-root 上 17 passed / 2 failed / 6 missing，Node/sshd 与缺输入维持原状态 |

初次常用工具检查误用了另一份 guest-root，`artifacts/tool-matrix-loader-entry.json` 为 6 passed / 1 failed / 18 missing，不能用于和既有 tool-root 基线比较。保留该报告，最终另用既有 tool-root 完整复测；Java 使用其实际安装所在的 guest-root 独立验收。

最终 runtime SHA-256 为 `00376566e571243a4ccd111d59bca8190768bf925c7a30bd2e04a4030a6e9a9e`，libdl 为 `d897164da1a16af48b44a66f7cd2bf02b63e1852c2a2e38457cb52394a8c1a0f`。最终入口 ELF、native math、Java 与工具矩阵均使用此 runtime；本批未重新运行完整 Docker。

剩余 libdl PE forwarder 和 ELF facade 仍存在，各 facade 的跳板地址可以不同；测试不将跳板地址相等当成 ABI 要求。物理独立 ld.so、其余 provider 去中转、真实 TCP/UDP procfs、Docker 跨容器 EIO/双栈/DNS/清理仍未完成。没有新增后台采集，没有测量 RSS 或吞吐收益。长期 goal 保持 active。


## 第十五批：去清单与原生导出装载（2026-09-16）

按最新约束，移除根模块清单、各模块私有 TOML、全局 exports/images TSV；不新增嵌入式清单。删除过渡方案中的 DLL 导入改写、标准库 HeapAlloc/HeapFree 重定向及 native_heap 模块。已弃用的实验产物不作为本批架构或验收依据。

- 构建使用模块自己的标准 `exports.def`；链接前合并 rustc 导出。DLL 的 `.so` 导入名由原生 COFF 导入库给出，打包仅复制原始字节。Linux 别名使用 PRIVATE，防止混入宿主导入库导致 Windows CRT 绑定到 SysV 函数。
- runtime/worker 从实际目录和 PE 导出表发现模块，按名称或精确 `name@VERSION` 获取原生地址。模块对象尺寸通过有界、借用输入的 C ABI 按请求取得，不猜测对象长度。
- 删除旧 ELF facade 的运行时映射与绑定层，所有 29 个对客体模块使用 PE `.so`。SDK ELF 只用于编译链接，并移除其中的完整模块 JSON 描述。图形扩展及 ld/libdl 仍有标准 PE 转发导出，尚未完成全部物理实现拆分。
- 清除无状态别名模块的 SDK 初始化/管理依赖。管理状态通过 runtime 的固定 C ABI 调用 init；没有另加模块描述入口或注册配置文件。
- 原生文件读取改用只读映射，由句柄持有并禁止映射期间改写文件，避免把完整 DLL 复制进私有 Vec。这里只验证了行为，没有 RSS、吞吐或极限性能测量。
- 打包在自有临时目录中完成校验，再逐文件原子发布；保留未知文件，删除只针对本次拥有的临时文件。整目录发布仍不是原子事务。

### 本批产物与验证

当前实验发行目录为 `artifacts/native-direct-dist`，只有 `host/` 和 `sdk/`。历史第十四批 `dist/` 未被新产物替换。

| 验证 | 结果 / 证据 |
| --- | --- |
| 完整 workspace 构建 | 通过，`artifacts/native-direct-clean-build.log`；仍有既有命名/unsafe 等编译警告，未宣称全工程无警告 |
| native / bridge / packager 检查 | 25 通过、1 个既有进程辅助用例忽略，`artifacts/native-direct-clean-unit.log` |
| 导出生成与来源检查 | `generate.py --check`、模块接线检查通过；19 ABI 工具测试、15 guest-deps 测试通过 |
| 无清单打包 | 29 个模块及原生依赖，`artifacts/native-direct-clean-package.log` |
| DLL 原样交付 | 36 个 `.so` 与源 DLL SHA-256 完全一致，`artifacts/native-direct-image-hashes.json` |
| 管理生命周期 | 两个 worker 的 Copy/Share/Reset 与清理通过，`artifacts/native-direct-clean-smoke.log`；退出后 processes/objects/transactions 均为 0 |
| Linux BusyBox | 真实 ELF 输出 DIRECT_NATIVE_OK，`artifacts/native-direct-busybox-verified.log` |
| Java 25 | `java -version` 退出 0，`artifacts/native-direct-java-version-verified.log`；不等于本轮 JIT/线程/ProcessBuilder 完整验收 |
| 基础程序哈希记录 | `artifacts/native-direct-basic-tools.json` |
| 真正的 guest fork | **未通过**：loader-entry 与 native-math 均到达 fork 后失败；退出码分别 92 / 1，未超时。见 `artifacts/native-direct-loader-report.json`、`artifacts/native-direct-math-report.json` |

### 未完成事项

管理 RPC 的状态交接通过，不能替代 guest 地址空间 fork。诊断中父进程已启动原生 child，协调器随后返回 `EAGAIN`；模块私有 Rust 状态恢复仍未完成。现有 fork 对 DLL 全局区和托管 Rust 堆的旧假设需继续按显式内存、内核句柄、序列化和模块恢复接口拆除；不恢复整个标准库堆重定向作为补救。旧的向 libc 注入 SDK 状态入口的专用 fixture 已移除，现有管理 smoke 和真实 ELF fork 探针分别保留其验收范围。

内部仍有 Rust dylib ABI 依赖，完整稳定 C ABI、物理 ld.so 拆分、模块私有状态恢复及常用工具/Docker/CUDA 的进一步验收继续推进。长期 goal 保持 active，不能标记完成。

## 第十六批：原生模块 fork 状态恢复与构建收敛（2026-09-17）

### 修复两个 fork 回归

第十五批 loader-entry/native-math 的稳定失败已修复。Windows 子进程调试定位到 TLS 切换区恢复向父进程普通 Vec 地址写入；修复该处后，装载器仍引用父进程私有 Box/Vec/HashMap，进一步导致子进程查询崩溃。

- TLS 块、ABI 切换区和宿主调用栈这些对外地址改为显式 runtime 分配。普通模块堆保持私有，不重定向标准库分配器。
- `kinakaze-link/src/linker/fork.rs` 单独负责有界二进制状态：对象、符号作用域、引用、句柄令牌、搜索路径及装载元数据。子进程重新获取模块引用、解析已恢复的不可变 ELF 数据，并重建私有 Rust 容器；不再发布父进程的 Linker 指针。
- 不可变 ELF 视图与映像映射通过 runtime 的映射登记、句柄槽和所有权转交接口恢复。原生模块重新装载并验证基址，恢复不重复执行客体构造器。
- 对客体公开的 `link_map` 及名称使用显式 runtime 内存，跨 fork 保持原地址；其余查询缓存与集合在子进程重建。libc 的复制重定向入口由 loader 从实际模块重新解析，删除外层 runtime 的直接 libc 符号导入。
- runtime 的 fork 子分支不再析构父进程的临时 participant Vec。仍有其他模块生命周期边界需要继续审计，不能据此宣布所有私有状态都已迁移完成。

loader-entry 探针增加继承 `link_map`/名称地址、子进程首次打开 libm、两代 fork、符号查找与引用释放。native-math 增加 fork 后创建线程、数学调用、errno 隔离及孙进程恢复。最终两者各运行 20 次、并发执行，共 **40/40 通过**。

### 清理与构建

- 工具目录改为 `tools/native-exports`；删除重复 frontend 同步脚本、libc 源码扫描器及生成的 TSV、无消费者的构建环境变量、仅用于计数的 runtime 导出文件。只读取真实 Cargo 依赖、标准 `.def` 和已编译 PE 导出，不新增模块清单。
- 构建开关改为 `-RefreshExports`。测试先按完整 Cargo 测试依赖图编译，再从 `target/<profile>/deps` 原样打包；普通发行 DLL 与测试 Rust ABI 不再混用。
- 现有内核/VFS 测试会修改进程级 PID、FD 与映射状态，构建入口按单测试线程执行；测试自身创建的并发线程、子进程与网络交互仍执行。此前并行测试的 5 个生命周期失败在串行验收中通过。
- 完整测试发现不存在的子路径与已 canonicalize 根路径前缀不一致，修复普通 DOS/UNC 与 verbatim 前缀的组件比较，保持路径边界检查。复制映射测试显式将当前 COW 文件视图物化为复制状态，再验证拒绝错误 discard、拆分与句柄回收。

### 最终验收

当前验收目录 `artifacts/native-direct-dist` 已更新；历史 `dist/` 保留。最终运行和哈希报告使用同一批发行产物。

| 检查 | 结果与证据 |
| --- | --- |
| 构建与完整默认测试 | `artifacts/fork-native-final-accepted-build.log`：`tools/build.ps1 -SkipFormat -DistDirectory artifacts/native-direct-dist` 退出 0；Rust 汇总 1,714 通过（包含被调用的子进程 helper）、0 失败、24 既有忽略；Python 18 native-exports + 15 guest-deps 通过 |
| 原生 DLL 原样交付 | `artifacts/fork-native-image-hashes.json`：36 个 `.so` 加 1 个工具链 DLL 与源 DLL 逐字节哈希相同 |
| 导出检查 | 29 个链接器输入、4,427 个客体导出、787 个 ELF 版本证据文件，`generate.py --check` 通过 |
| 真实 ELF 两代 fork | `artifacts/fork-native-final-concurrent.json` / `.log`：20 轮并发、40 次通过，无超时，保留每次源码/ELF/worker/runtime 哈希和标准错误 |
| 单次探针报告 | `artifacts/fork-native-loader-report.json`、`artifacts/fork-native-math-report.json`，退出 0 |
| 管理回收 | 最终构建日志的 smoke：两个 worker Copy/Share/Reset 通过，退出后 processes/objects/transactions 为 0 |

最终 runtime SHA-256 为 `3cf6fef9a1a36f3cd16a433ddf3303f9f24672a81398bf3afb2f9718b6b1f9e4`，worker 为 `42760ed3f9cc2733e414a22ab1a657ebdedb5ee0cf0784825393009431fc1065`。本次未执行全仓格式检查；修改的 Rust 文件已格式化，diff 检查通过。编译仍有既有警告。24 个忽略项包含硬件/性能诊断、被父测试调用的 helper，以及既有 verity/overlay 行为缺口，不能将其视为全部通过。

中间构建的一次并发 native-math 运行出现进程表不可用日志及子进程状态失败，见 `artifacts/fork-math-accepted.log`。随后该中间构建的 5 轮并发和最终构建的 20 轮并发均通过；尚未定位该单次异常，保留跟踪，不能宣称所有生命周期竞态已消除。最初测试 DLL 混用错误和两轮失败测试日志也保留，不覆盖为成功记录。

本批未新增后台采集或生产环境开关，没有测量 RSS/吞吐收益。物理 ld.so、内部稳定 C ABI、其余模块状态恢复、Java/Node/sshd/Docker/CUDA 的新架构验收继续推进；历史完整应用结果不外推。长期 goal 保持 active。

## 批次 17：实际 ld.so 独立交付（2026-09-17）

实际 ELF 链接、重定位、映像、符号查找、dl* 和装载器 fork 状态已迁至
`libs/ld-linux-x86-64/src`，直接编译为 `kinakaze_link.dll` 并原样交付为
`ld-linux-x86-64.so.2`。删除 `engine/crates/kinakaze-link` 和 engine 内的
`host/loader`，没有 implementation 子层或新的模块清单。

dlopen/dlsym/dlvsym/dlclose/dlerror/dlinfo/dladdr/dl_iterate_phdr 的实际定义均在
ld.so；libc/libdl 的别名直接指向这一物理实现。engine 的 fork 状态只负责
TLS/TEB/ABI 切换，ld.so 自己登记、序列化并恢复 Linker。构造器调用通过 runtime
登记的服务接口进入所属模块。修正 dladdr 将转发别名误归属到其他 PE 映像的问题，
删除动态查找的 KINAKAZE_DL_TRACE 开关。

loader-entry 增加真实 ELF DSO，验证 ELF/PE 混合句柄、唯一入口归属、构造器只运行
一次、每线程 TLS 及两代 fork。新探针对旧发行包失败，拆分后通过。证据：
`artifacts/native-ld-loader-before.log`、`native-ld-full-build.log`、
`native-ld-final-fork.json`（5 轮配对，10/10 通过）、`native-ld-native-images.json`。
此后最终发行包由下述批次 18 更新，旧批次哈希不代表当前产物。

libdl 的 PE 兼容转发仍存在，内部 Rust dylib ABI 尚未全部改成稳定 C ABI，不能将
这次物理拆分等同于所有模块边界已完成。

## 批次 18：补齐工具 ABI 与外部依赖（2026-09-17）

Node 行为测试与 sshd 启动测试现已通过。GCC 的 cc1 已能编译输入，仍在 collect2
查找链接器阶段失败；Redis/PostgreSQL 已得到完整 ELF 依赖闭包，尚有 ABI 缺口。

### 已实现与修复

- `setfsuid/setfsgid` 和 x86-64 原始系统调用 122/123：按 namespace 映射权限判断，
  返回旧 ID 且保留 errno；与有效 ID 更新同步，序列化 fork/exec 身份，文件系统
  创建/检查读取 fsuid/fsgid。非文件系统身份查询仍使用有效 ID。
- `backtrace/backtrace_symbols/backtrace_symbols_fd`：libc 汇编入口先捕获客体寄存器，
  ld.so 通过 gimli 解析真实 ELF `.eh_frame_hdr`/CFI，按调用展开并符号化。保存的栈字
  通过有界主进程读取取得；返回字符串使用一块客体分配。无后台采样或栈地址猜测。
- 修复 Node 启动的 COPY/RELRO 顺序：先创建并发布初始栈，再重定位与封页。
  `CopiedInt/Pointer` 内部可变载荷改用 UnsafeCell，保持客体 ABI 前缀布局。
- 支持 jemalloc 使用的 STN_UNDEF 本地 TLS 偏移重定位；未触及具名符号解析策略。
- `MADV_DONTNEED` 接受仍未提交的匿名 placeholder，保留零填充语义，修复 V8 创建
  JIT guard page 时失败。read/write 的零长度 NULL 缓冲不再构造非法 Rust slice；
  负文件描述符写入返回真实错误，删除假成功路径。
- 补齐 futimes、getdomainname、inet_addr、文件型 innetgr、utmp/utmpx 查询别名与
  login/logout/logwtmp、线程名称接口及 rwlock 属性、ffs 家族、vasprintf/asprintf、
  obstack 格式化和 warn 家族。`__syslog_chk/__vsyslog_chk` 进入现有实际日志投递路径。
- 新 `mallinfo2` 按需锁定并读取真实 guest allocator 的 arena、空闲链和线程缓存，
  不增加分配热路径计数或闲置采集；未使用的 guest 堆不会因查询被初始化。
- 将旧 glob 的“直接返回原模式字符串”实现拆到独立 `glob.rs`，增加真实 VFS 展开、
  隐藏文件过滤、排序、NOCHECK、目录标记、DOOFFS、APPEND、释放及 glob64 ABI。

### 依赖与结构

新增 54 个锁定 Debian 包，依赖锁现共 63 包，包括 GCC 12.2、binutils 2.40、
开发头文件/启动对象、Redis 7.0.15、PostgreSQL 15.19 及所需共享库。下载按官方包索引
SHA-256 校验，保留包与源码版本、实际链接目标和许可证；不运行维护脚本。
跨包命令别名使用实际文件所有者的字节；编译器 ET_REL 对象作为编译输入处理。
头文件与 PostgreSQL 资源使用已有经过包哈希认证的 tree 机制，避免大量重复文件记录。
GCC 的 cpp、binutils 和开发输入通过包依赖自动准备。

`artifacts/libc-gap-locked-preflight.log`：离线检查 1,858 个 GCC/Redis 输入与 536 条
依赖边；`libc-gap-postgres-root5.log`：PostgreSQL 根目录 1,631 个文件、390 条依赖边。
这些是输入闭包结果，不是数据库行为验收。外部包锁与来源证据不参与原生模块发现。

### 最终验证

| 检查 | 结果与证据 |
| --- | --- |
| 全仓构建及默认测试 | `artifacts/libc-gap-full-build.log`：Rust 1,715 通过（含 helper 子进程）、0 失败、24 既有忽略；Python 18 native-exports + 16 guest-deps 通过 |
| 新 ABI 实际 ELF | `artifacts/libc-gap-probe/report.json`、`libc-gap-final-probe.log`：优化且省略帧指针的 DWARF 回溯、fork 后恢复、文件身份/未链接 inode 时间、netgroup/domain/IPv4、NULL 零长度 IO、匿名 guard discard、真实堆统计、glob 与混合整数/SSE 变参通过 |
| 装载与 fork | `libc-gap-final-loader.log`、`libc-gap-final-math.log`：loader-entry/native-math 均通过，覆盖两代 fork 与真实 ELF DSO |
| 常用工具 | `artifacts/libc-gap-final-tools.json`：19 通过、6 个该根目录缺少的输入；原 Node/sshd 失败转为通过；shell/Bash、Python、curl/wget、ssh、dpkg/apt、SQLite 原有通过项保留 |
| 原生 DLL 原样交付 | `artifacts/libc-gap-final-images.json`：35 个 SO + 1 个工具链 DLL 与编译来源哈希一致；8 个 dl* 实际定义仅在 ld.so |
| 导出与退出回收 | 29 个链接输入、4,478 个客体导出、1,141 个版本证据 ELF；smoke 退出后 processes/objects/transactions 均为 0；diff 检查通过 |

最终包为 `artifacts/native-direct-dist`，runtime SHA-256
`94f9e12af2121dd43b50ad6c67e98d4c25ca4c1cc072f7c61d635fe50d9dfe92`，
ld.so 为 `9f46edb986c912a9d38734e35499639b31c51ee2788dca5ce9fac9344d2ee5e3`。
本批修改的 Rust 源码已格式化；未执行全仓格式检查，仍有既有编译警告。

### 明确保留的边界与下一项

- sshd 这里只验证启动，未验证认证、登录会话和服务生命周期；Node 本轮不覆盖所有
  原生扩展和子进程模式。
- `libc-gap-final-compiler-db.json`：GCC 停在 collect2 的 ld 查找，尽管 `/usr/bin/ld`
  文件存在且 `ld --version` 成功；Redis 缺 `llroundl@GLIBC_2.2.5`。
  `libc-gap-final-postgres.json`：PostgreSQL 缺 `towupper_l@GLIBC_2.3`。
  继续处理这些具体失败，不将库闭包或编译阶段成功写成完整程序支持。
- Clang/FFmpeg 输入尚未补齐。Java JIT/线程/文件在批次 17 通过，但默认 ProcessBuilder
  与独立 posix_spawn 失败；spawn 仍持有待迁移的模块私有容器，继续处理其 fork 边界。
- DWARF 展开遇到缺少索引、表达式规则或原生边界会停止；不宣称完整异常展开/JIT CFI。
  glob 的自定义目录回调、brace/tilde 扩展明确拒绝；rwlock 的进程共享初始化返回
  ENOTSUP，避免将私有 Rust 锁伪装成进程共享锁。UTS/账户/locale 等其他既有边界仍在。
- 未测量 RSS、吞吐或 Unix socket 性能收益；未新增后台采集。长期 goal 保持 active。
