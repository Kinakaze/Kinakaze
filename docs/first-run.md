# 首次安装与包管理

宿主使用 Windows 11 24H2 / Windows Server 2025 及以上的 x86-64 系统；原生 I/O 依赖新版 Windows IoRing API。

Windows x86-64 运行包解压后，双击 `worker.exe`，或在 PowerShell 中执行：

```powershell
.\worker.exe
```

第一次启动读取同目录的 `rootfs.manifest.json`，自动从 Debian 软件源下载默认包、解包并配置 `rootfs`，随后进入登录 shell。下载与安装由 EXE 内置完成；用户只需 release 文件和网络，不需要 Python、Rust、外部解压工具或另找 Linux 根目录。VC++ 运行库 DLL 随包提供。双击 `init.exe` 会启动同一环境并打开终端。

启动窗口实时显示安装日志：包下载字节数和百分比、已完成包数、缓存复用、解包、文件写入及启动阶段。没有新日志时每 5 秒显示等待时长；失败原因直接显示在窗口中。日志同时保存在窗口提示的 `init.log` 路径，重新启动不会重放上一次的旧日志。

现在默认启动一个常驻托盘 `init.exe`，同一 rootfs 再次启动会复用它。`Ctrl+]` 或关闭终端窗口只断开交互；托盘右键可回到原 Bash/ELF、打开 WebUI，或关闭整个进程树。也可运行 `worker.exe session stop` 关机。PID 1、开机脚本、应用和关机命令均由 manifest 配置，核心不绑定 systemd；见 [持久会话配置](persistent-sessions.md)。

运行包最外层只放 `init.exe`、`worker.exe`、所需 DLL、`native/` 原生模块和默认 Debian 清单；许可材料放在 `licenses/`，运行包不附带说明文档。运行包不携带预装 rootfs 或离线种子。首次下载与客体 APT 默认使用中科大 Debian 镜像（`https://mirrors.ustc.edu.cn/debian` 和 `https://mirrors.ustc.edu.cn/debian-security`），可在国内直连。首次下载仍自动跟随 Windows 系统代理，包括手动代理、PAC/WPAD 自动配置和代理绕过规则；需要强制直连时可在 manifest 设置 `proxy: "direct"` 或用下述环境变量覆盖。下载缓存保存在 rootfs 同级的 `.kinakaze-downloads/`；下载失败可重新启动，已经下载成功的包会复用。全部配置成功后才发布 rootfs。

默认环境包含 Debian bookworm 的无桌面基础系统和标准工具：按 Essential、required、important、standard 选出 103 个初始包，加入 OpenSSH 服务端，再由 APT 补齐依赖与默认推荐，共锁定 306 个软件包。包括 Bash、补全、man、nano、vim-tiny、less、Python、Perl、网络和进程工具，以及 APT、dpkg、签名密钥、证书、共享库和软件源。登录后位于 `/root`；BusyBox 提供缺少的基础命令。进入 shell 后可以使用：

```sh
apt-get update
apt-get install hello
hello
apt-get purge hello
# bzip2 已经预装
printf hello | bzip2 | bzip2 -d
```

默认软件源为 Debian bookworm、bookworm-updates 和 bookworm-security 的 amd64/main，使用 HTTPS 和 `Signed-By` 密钥验证。首次配置联网下载清单锁定的 Debian 包并校验 SHA-256；APT 软件包索引在执行 `apt-get update` 时下载。预制安装已包含默认推荐依赖；后续交互安装默认不增加推荐或建议包，需要时可用 `--install-recommends`。缓存初始容量为 64 MiB，位于 `/etc/apt/apt.conf.d/00kinakaze`，可按自定义软件源规模调整。

首次安装默认使用 Windows 系统代理，不把本机代理地址写入清单。安装后，若客体内的 APT 也需要代理，可在 `/etc/apt/apt.conf.d/90proxy` 中配置，例如 `Acquire::https::Proxy "http://proxy.example:8080";`。默认清单不包含开发机器的代理或本地路径。

## 默认 SSH 登录

打开默认登录 shell 后，OpenSSH 服务端自动启动，仅监听本机 `127.0.0.1:2222`。在另一个 Windows 终端连接：

```powershell
ssh -p 2222 root@127.0.0.1
# 密码：kinakaze
sftp -P 2222 root@127.0.0.1
```

默认使用密码认证，也保留公钥认证。首次启动自动生成独立的 RSA、ECDSA、ED25519 主机密钥；后续启动保留已有密钥、密码和配置。主机私钥与 `/etc/shadow` 权限为 0600。`setup` 只安装环境；默认开机配置由 systemd 启动 SSH。断开终端或退出 Bash 不关闭 SSH，关闭整个环境才停止服务。

配置写在清单的 `files` 中：`etc/ssh/sshd_config` 设置监听地址、端口和认证，`etc/shadow` 保存初始密码哈希。首次安装前可编辑清单，安装后可修改客体配置或执行 `passwd`。默认 systemd 配置使用 `systemctl disable --now ssh.service` 禁用服务；`etc/default/kinakaze-sshd` 的 `ENABLED` 仅控制兼容登录脚本的自动启动。日志保存在 `/var/log/sshd.log`。

## systemd 会话

双击 `init.exe` 会完成同样的首次联网配置，生成独立且持久的 machine-id，然后让 Debian systemd 作为客体 PID 1 启动 `kinakaze.target`。默认目标启用 `ssh.service`，自动生成主机密钥，仍通过 `root@127.0.0.1:2222`、密码 `kinakaze` 连接。其他服务可按需启用。打开 SSH 交互终端后可以执行：

```sh
systemctl is-system-running
systemctl status ssh.service
systemctl restart ssh.service
```

`systemctl stop ssh.service` 会停止监听；可从托盘返回开机 Bash 后重新启动服务。`init.exe` 和普通入口现在都遵循 manifest，共用一个常驻环境；关闭终端窗口只断开连接，托盘“关闭整个进程树”才关机。默认配置的自启由 `systemctl enable/disable ssh.service` 管理，日志仍写入 `/var/log/sshd.log`。

这是运行在 Windows ABI 兼容层中的 Debian 用户空间。默认目标和 SSH 使用 `DefaultDependencies=no`，由宿主完成运行环境初始化，不启动 Debian 的整套硬件、内核和日志服务。添加应用服务时可采用相同配置，并将输出写入文件；原样启动 `multi-user.target`、journald、udev 等不在本次验收范围。已验证 PID 1、服务通知、服务启动/重启/停止和 SSH 交互登录；需要专有内核接口或设备驱动的 unit 仍须单独验证。

## 下载镜像与代理

首次启动前，可直接修改 release 中 `rootfs.manifest.json` 顶部的 `download` 配置：

```json
"download": {
  "debian_mirror": "https://mirrors.ustc.edu.cn/debian",
  "security_mirror": "https://mirrors.ustc.edu.cn/debian-security",
  "proxy": "system"
}
```

将两个镜像地址改为目标镜像的 Debian 仓库根地址即可，包路径自动保留，不需要逐个修改 `archives`。普通仓库和安全更新仓库分别设置；镜像必须提供清单锁定版本的包，下载仍校验大小与 SHA-256。镜像使用 HTTPS，也允许 `http://127.0.0.1:端口/路径` 本地源。

`proxy` 支持以下值：

- `"system"`：默认，自动跟随 Windows 系统代理与绕过规则。
- `"direct"`：禁用代理，直接连接镜像。
- `"http://127.0.0.1:7890"`：指定 HTTP 代理，也用于 HTTPS 下载的 CONNECT 隧道。地址不支持内嵌账号密码或 SOCKS 协议。

也可以通过 PowerShell 环境变量覆盖，无需编辑清单：

```powershell
$env:KINAKAZE_DEBIAN_MIRROR = 'https://mirror.example/debian'
$env:KINAKAZE_DEBIAN_SECURITY_MIRROR = 'https://mirror.example/debian-security'
$env:KINAKAZE_DOWNLOAD_PROXY = 'http://127.0.0.1:7890'
# 禁用代理时改为 'direct'；跟随系统时改为 'system'。
.\worker.exe
```

示例 `mirror.example` 需替换成实际镜像地址。每个设置的优先级为：非空 `KINAKAZE_*` 环境变量 > manifest > 默认值。Windows 首次安装不读取通用 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 变量，避免它们意外覆盖清单中的直连设置。清空对应环境变量即可恢复清单设置。

这里的镜像和代理配置用于首次 rootfs 下载，不改写已安装系统中的 APT 配置。要同时调整安装后的 APT 软件源，可在首次启动前修改清单 `files` 中 `etc/apt/sources.list.d/debian.sources` 的文本内容，或安装后编辑该文件。已有非空 rootfs 不会重新配置；下载失败后修改镜像或代理再启动，会继续复用已经校验成功的缓存。

从旧版默认源切换已有 rootfs 时，在客体 Bash 内执行以下命令；仅替换原默认站点，保留 suite、组件和签名验证配置：

```sh
sed -i 's@https://deb.debian.org/@https://mirrors.ustc.edu.cn/@g' /etc/apt/sources.list.d/debian.sources
apt-get update
```

## 指定 rootfs 或清单

```powershell
# 联网下载并初始化，不启动客体进程。
.\worker.exe setup --root D:\KinakazeRoot

# 使用这个 rootfs 进入 shell。
.\worker.exe --root D:\KinakazeRoot

# 外部清单优先于发行目录中的默认清单。
.\worker.exe --root D:\CustomRoot --rootfs-manifest D:\Image\rootfs.manifest.json

# 直接运行命令；退出码向调用方传递。
.\worker.exe --root D:\KinakazeRoot -- /usr/bin/apt-get update
```

manifest 可选。只有已选择清单，且 rootfs 不存在或为空目录时才初始化。任何已有条目都会使初始化跳过，且不再打开清单；重复启动不会覆盖配置或已安装的软件包。没有清单时仍可使用自行准备的 rootfs。

清单采用 schema 1，支持 `directories`、文本/哈希文件 `files`，以及可选的 `permissions`、`links` 映射和 `case_sensitive` 标志。权限值为十进制 Linux mode，例如 493 表示 0755，420 表示 0644；`/` 表示根目录，其余权限路径必须在清单中声明。`links` 为客体路径到链接目标的映射；链接不能作为其他安装条目的父目录。`archives` 记录 Debian 包的 HTTPS 地址、大小与 SHA-256；文件条目可用 `archive`、`member`、`sha256` 指定包内文件，安装器直接解析 Debian ar 与 tar.xz/tar.gz，无需外部工具。没有 `archives` 的旧离线清单仍然可用。完整 Debian 镜像开启目录大小写敏感，Windows 存储卷必须支持该功能（本次使用 NTFS）。初始化以 root:root 写入权限，并在全部文件、链接和权限成功后原子发布目录。

## 基础环境的维护

Kinakaze 原生 ABI 和锁定 Debian 软件包的文件统一登记为 Essential 包 `kinakaze-base`，包含真实文件清单、校验值、配置文件记录和版本化 Provides。`dpkg-query -L kinakaze-base` 可查看其文件；`/usr/share/kinakaze/bootstrap-packages.json` 记录各组件的锁定来源，`/usr/share/kinakaze/debian-standard.json` 记录选择规则与原生 ABI 替换等适配。编辑器、分页器等默认选择写入真实的 `update-alternatives` 数据库。

预装软件文件不等于启用了所有 Debian 服务。`policy-rc.d` 阻止安装包时自动启动守护进程；systemd、内核、设备和硬件工具仍受运行时能力限制。含字面反斜杠的一个 systemd slice 文件暂不安装，具体路径记录在适配清单。这是一套适配 Kinakaze 的基础环境，未将上游维护脚本没有执行过的组件冒充为独立安装完成的 Debian 包。基础环境随 Kinakaze 发行包更新；不要强制卸载 `kinakaze-base` 或强制覆盖其文件。后续通过 APT 安装的包由 dpkg 正常记录、配置和卸载。

## 从源码构建

```powershell
./tools/build.ps1 -Release -DistDirectory artifacts/first-run-dist
python tools/test-first-run.py --dist artifacts/first-run-dist
python tools/test-first-run.py --dist artifacts/first-run-dist --network
```

Release 构建生成默认在线安装清单，首次使用时联网。调试构建保留离线种子模式。`-Offline` 只控制构建阶段是否使用本地依赖缓存，不改变 Release 用户端的在线安装模式；`-NativeOnly` 仅构建原生模块。`config/rootfs.packages.json` 管理预装策略、alternatives 和 BusyBox 补充命令；`config/debian-standard.lock.json` 固定标准安装的归档 SHA-256、版本和已验证软件源索引。更新选择可运行 `tools/lock-debian-standard.py --dist <发行目录> --root <已完成 apt-get update 的 rootfs>`。自定义精简镜像可移除 `debian_standard_lock`，继续使用原先的逐文件依赖锁。修改预装配置后请选择新的发行目录，构建器会拒绝覆盖已使用或已修改的 rootfs。

运行包携带入口程序、依赖 DLL、`native` 原生模块与默认清单，首次启动时即可选择自定义 rootfs。ZIP 无法保留 NTFS 中的 Linux inode 元数据，因此 rootfs 在首次启动时下载并创建，权限通过安装器恢复。用户无需安装宿主 Python；默认会话服务使用 Debian 内自带的 Python，宿主入口仍全部为原生 EXE。

版本标签发布流程会构建 Windows 运行包，验证首次启动及 ZIP 解压运行，再与源码 ZIP 一起上传到同一个 GitHub Release。运行包和源码包均附 SHA-256。手动生成候选包可执行：

```powershell
python tools/test-release-runtime.py --dist artifacts/first-run-dist --report artifacts/runtime-validation.json
python tools/package-runtime.py --dist artifacts/first-run-dist --report artifacts/runtime-validation.json --preview
```

## 验证记录

当前标准预装扩展的验证见 `docs/debian-standard-validation.md`。以下是上一轮精简环境的验证记录。

2026-09-26 在 Windows x86-64 开发主机验证了 48 个锁定组件、793 个安装文件和 584 条依赖关系组成的默认环境。主机已安装 Visual C++ 运行库；测试清空了 PATH，并使用全新临时 rootfs。

- APT 从默认 HTTPS 软件源校验签名、安装并运行 Debian `hello` 和 `bzip2`，完成压缩还原与卸载；本地依赖包的维护脚本也正常执行。
- 宽字符 printf 的文件流、Unicode 计数、位置参数、浮点、栈参数和 checked/va_list 入口通过真实 ELF 验证；两个观察 ELF 的 70 条版本需求无缺失。checked 流接口与现有窄字符实现一致，尚未实现 GNU FORTIFY 对可写格式串中 `%n` 的额外限制。
- 通过 8 项首次启动/进程树验收、12 项 rootfs/元数据测试、43 项 Python 工具测试、全目标编译及格式检查。
- 实际解压候选 ZIP 后，首次指定独立 rootfs、离线 APT 依赖安装/卸载、默认 `worker.exe` 登录 shell 和退出码传递均通过。

本地候选包使用 `--preview` 标记，记录工作区状态和文件哈希；它没有替换已发布的 v0.1.0。验证覆盖上述程序与行为，不代表全部 Debian 软件均已兼容。
