# Debian 标准命令行预装与 ABI 验证

默认清单按 Debian bookworm amd64 的 Essential、required、important、standard 选择 103 个初始包，再用真实 APT 的空状态求解补齐依赖和推荐，另加 BusyBox、curl、zip、unzip、tree，共 302 个锁定软件包。这对应无桌面的基础与标准命令行范围；包优先级的含义见 [Debian FAQ](https://www.debian.org/doc/manuals/debian-faq/pkg-basics.en.html)。选择、版本、官方来源和 SHA-256 见 `config/debian-standard.lock.json`。

初始化恢复完整归档文件、符号链接、权限和大小写敏感目录。原生库替换相同 SONAME 的上游实现，所有适配记录在客体 `/usr/share/kinakaze/debian-standard.json`。默认 Bash 登录进入 `/root`，提供补全、man、编辑器、分页器、Python、Perl 和 APT。启动仍遵守“manifest 可选、外部优先、仅初始化不存在或空目录”的约定。

## ABI 变更

新增 56 个导出：libc 52 个、libpthread 2 个、libm 2 个。主要包括 hosts/protocols/services/networks/RPC/passwd/group 枚举及其 `_r` 入口、`dprintf`/`vdprintf`/`vprintf`、`__vdprintf_chk`、`_nl_msg_cat_cntr`、`sockatmark`、`explicit_bzero`、`remquo`/`remquof`，以及已有实现的兼容别名。

- 小缓冲区返回 ERANGE 后保留数据库枚举位置；别名数组和地址数组完整终止，支持未对齐缓冲区。
- `_nl_msg_cat_cntr` 按 Linux int 导出，COPY 重定位和 fork 后重定向保持一致。
- 自旋锁改为 Linux ABI 的 4 字节存储，并补齐现代 libc 导出，避免写坏相邻数据。
- 已存在的 `/etc/passwd`、`/etc/group` 为账户查询的依据，不再向其中注入硬编码账户；错误的 UID/GID 不再解释为 root。
- `msgget`、`msgsnd`、`msgrcv` 可供程序装载，并明确返回 ENOSYS，消息队列本身尚未实现。
- 补齐 `getent` 的 13 个入口：NSS 源选择、动态数组扩容、aliases、gshadow 和 ethers；支持真实文件数据库、连续行、管理员/成员列表与组影子记录输出。默认生成 0600 的 `/etc/gshadow`。NSS 源限定为已实现的 files，以及 hosts 的 files/dns；未实现的源明确报错。
- netlink 支持 SO_SNDBUF/SO_RCVBUF、发送/接收预算和 fork 继承；无显式目的地址时发往内核，支持 PEEK/TRUNC 长度探测，multipart DONE 携带完成状态，并接受 iproute2 在消息尾部发送的零填充。无 Linux qdisc 的宿主接口报告零长度发送队列，避免触发不支持的 ioctl 回退。
- `/proc/<pid>/cwd`、`root` 使用跨进程目录状态，支持 chdir/fchdir/chroot、fork 和链接下的路径访问；openat2 的禁止 magic-link 标志仍生效。
- 直接启动先解析客体符号链接，支持 Debian 的 `/bin/sh`、`/sbin/ip` 等入口；exec 的已固定镜像仍保留原有语义。

## 可复现验证

```powershell
./tools/build.ps1 -Release -DistDirectory artifacts/debian-standard-release -Offline
python tools/test-debian-cli.py --dist artifacts/debian-standard-release
python tools/test-first-run.py --dist artifacts/debian-standard-release --network
python tools/test-release-runtime.py --dist artifacts/debian-standard-release
python tests/guest/run-daily-tools.py --dist artifacts/debian-standard-release --worker artifacts/debian-standard-release/worker.exe --link-dir target/release/elf-imports
python tests/guest/run-standard-abi.py --dist artifacts/debian-standard-release --worker artifacts/debian-standard-release/worker.exe --link-dir target/release/elf-imports
```

ABI 探针检查多线程竞争、4 字节自旋锁边界、账户与网络数据库、ERANGE 重试、fork、COPY 数据、SysV GP/SSE/栈参数、文件描述符错误、清零及余数和商。CLI 和安装验收使用新临时 rootfs，并清空宿主 PATH。打包校验以各次报告中的文件哈希为准。

2026-09-26 的最终预览构建通过了 10 项命令行验收、6 项首次安装与 APT 验收（含 HTTPS 签名索引、安装并运行 hello、卸载），以及 8 项初始化与进程树验收。DailyTools、StandardAbi、NetworkDb、ServicesDb、ProcnetRoute 五组真实 ELF 探针全部通过；`ip -j addr` 返回接口和地址且标准错误为空。三个目标命令的 303 项版本化导入要求无缺失。

构建还通过了 workspace 全目标检查、格式检查、13 项 rootfs 测试和 43 项 Python 工具测试。最终清单包含 16,054 个文件及 3,120 个客体符号链接；离线安装数据和 201 个上游源码包一同纳入预览包。验收报告合并到 `artifacts/bootstrap-metadata/standard-validation.json`，记录被测镜像的 SHA-256，打包时再次核对。

## 当前边界

预装文件不代表所有命令已兼容。`lsof` 的 cwd/root/exe 和普通文件链接已补齐；跨进程匿名管道/套接字的完整 stat 信息仍有缺口，不能把它等同于完整 Linux procfs。系统服务、设备和完整 Linux 内核行为仍需继续补齐。完整 ELF 扫描包含尚不支持的 GLIBC_PRIVATE 和旧 RPC 接口，不能据此宣称全 Debian ABI 完成。

时区数据已预装，当前运行时解析 POSIX TZ 规则，尚不解析 IANA TZif 文件。默认登录设置 `TZ=UTC0`；自定义时区可提供 POSIX TZ 字符串。预装包以适配包 `kinakaze-base` 统一登记，上游维护脚本未整体执行；后续新增 APT 包正常执行维护脚本，`policy-rc.d` 阻止自动启动服务。详见 [首次安装说明](first-run.md)。

本轮工作使用本地 preview 标识，没有覆盖已发布的 v0.1.0。
