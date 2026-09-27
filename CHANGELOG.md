# 更新记录

## 0.2.0 — 2026-09-27

- Windows 运行包携带 EXE、依赖 DLL 和 Debian 清单，首次启动自动联网安装，无需宿主 Python；默认使用中科大镜像，支持系统代理、清单配置与环境变量覆盖，实时显示下载和安装进度。
- 默认 Debian bookworm 环境包含 306 个锁定软件包；内置 OpenSSH 服务，默认监听 `127.0.0.1:2222`，账号 `root`、密码 `kinakaze`。
- 同一 rootfs 共用一个常驻进程树，客体 PID 1、启动流程与关机命令由清单指定；默认配置由 systemd 启动 SSH 和会话服务，支持自定义 init。
- 增加托盘管理、可断开重连的 Bash/ELF 交互终端与 WebUI 入口；修复鼠标控制序列污染终端输入，缩短关闭进程树时的等待。
- 改进原生镜像加载与启动性能，补齐 libc 正则、文件系统、socket、定时器、信号与 X11 行为，并扩充回归验证。
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
