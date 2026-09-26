# 首次安装与包管理

Windows x86-64 运行包解压后，双击 `kinakaze.cmd`，或在 PowerShell 中执行：

```powershell
.\kinakaze.cmd
```

第一次启动从同目录的 `rootfs.manifest.json` 和 `rootfs-seed` 离线创建完整 rootfs，随后进入登录 shell。无需另找 Linux 根目录，也无需先安装 Python、Rust、Java 或编译器。Windows 主机仍需提供 Microsoft Visual C++ x64 运行库。

默认环境包含 BusyBox shell、基础命令、APT、dpkg、gpgv、Debian 签名密钥、CA 证书、共享库、包数据库和软件源。进入 shell 后可以使用：

```sh
apt-get update
apt-get install bzip2
printf hello | bzip2 | bzip2 -d
apt-get purge bzip2
```

默认软件源为 Debian bookworm、bookworm-updates 和 bookworm-security 的 amd64/main，使用 HTTPS 和 `Signed-By` 密钥验证。软件包列表在首次执行 `apt-get update` 时下载；初始化本身不联网。APT 默认不安装推荐或建议包，需要时可用 `--install-recommends`。缓存初始容量为 64 MiB，位于 `/etc/apt/apt.conf.d/00kinakaze`，可按自定义软件源规模调整。

需要代理时，在客体 `/etc/apt/apt.conf.d/90proxy` 中配置自己的代理地址，例如 `Acquire::https::Proxy "http://proxy.example:8080";`。默认清单不包含开发机器的代理或本地路径。

## 指定 rootfs 或清单

```powershell
# 只初始化，不启动客体进程。
.\kinakaze.cmd setup --root D:\KinakazeRoot

# 使用这个 rootfs 进入 shell。
.\kinakaze.cmd --root D:\KinakazeRoot

# 外部清单优先于发行目录中的默认清单。
.\kinakaze.cmd --root D:\CustomRoot --rootfs-manifest D:\Image\rootfs.manifest.json

# 直接运行命令；退出码向调用方传递。
.\kinakaze.cmd --root D:\KinakazeRoot -- /usr/bin/apt-get update
```

manifest 可选。只有已选择清单，且 rootfs 不存在或为空目录时才初始化。任何已有条目都会使初始化跳过，且不再打开清单；重复启动不会覆盖配置或已安装的软件包。没有清单时仍可使用自行准备的 rootfs。

清单采用 schema 1，支持 `directories`、文本/哈希文件 `files`，以及可选的 `permissions` 映射。权限值为十进制 Linux mode，例如 493 表示 0755，420 表示 0644；`/` 表示根目录，其余权限路径必须在清单中声明。初始化以 root:root 写入权限，并在全部文件和权限成功后原子发布目录。

## 基础环境的维护

Kinakaze 原生 ABI 和经过筛选的 Debian 文件统一登记为 Essential 包 `kinakaze-base`，包含真实文件清单、校验值、配置文件记录和版本化 Provides。`dpkg-query -L kinakaze-base` 可查看其文件；`/usr/share/kinakaze/bootstrap-packages.json` 记录各组件的锁定来源。

这是一套适配 Kinakaze 的基础环境，未将上游维护脚本没有执行过的组件冒充为独立安装完成的 Debian 包。基础环境随 Kinakaze 发行包更新；不要强制卸载 `kinakaze-base` 或强制覆盖其文件。后续通过 APT 安装的包由 dpkg 正常记录、配置和卸载。依赖完整 Linux 内核或服务管理器的软件仍取决于 Kinakaze 的运行时支持范围。

## 从源码构建

```powershell
./tools/build.ps1 -Release -DistDirectory artifacts/first-run-dist
python tools/test-first-run.py --dist artifacts/first-run-dist
python tools/test-first-run.py --dist artifacts/first-run-dist --network
```

普通构建会生成完整默认清单。`-Offline` 使用已验证的依赖缓存；`-NativeOnly` 仅构建原生模块。`config/rootfs.packages.json` 管理预装包和 BusyBox 命令，归档及逐文件哈希由 `tools/guest-deps/dependencies.lock.json` 锁定。修改预装配置后请选择新的发行目录，构建器会拒绝覆盖已使用或已修改的 rootfs。

运行包携带入口程序、`native` 原生模块、默认清单和去重种子，首次启动时即可选择自定义 rootfs。ZIP 无法保留 NTFS 中的 Linux inode 元数据，因此 rootfs 在首次启动时创建，权限通过安装器恢复。准备正式运行包前，执行 `tools/update-release-sources.py --dist <发行目录>` 更新对应源码锁；打包器会核对源码集合与预装组件一致。

## 本次验证

2026-09-26 在 Windows x86-64 开发主机验证了 48 个锁定组件、793 个安装文件和 584 条依赖关系组成的默认环境。主机已安装 Visual C++ 运行库；测试清空了 PATH，并使用全新临时 rootfs。

- APT 从默认 HTTPS 软件源校验签名、安装并运行 Debian `hello` 和 `bzip2`，完成压缩还原与卸载；本地依赖包的维护脚本也正常执行。
- 宽字符 printf 的文件流、Unicode 计数、位置参数、浮点、栈参数和 checked/va_list 入口通过真实 ELF 验证；两个观察 ELF 的 70 条版本需求无缺失。checked 流接口与现有窄字符实现一致，尚未实现 GNU FORTIFY 对可写格式串中 `%n` 的额外限制。
- 通过 8 项首次启动/进程树验收、12 项 rootfs/元数据测试、43 项 Python 工具测试、全目标编译及格式检查。
- 实际解压候选 ZIP 后，首次指定独立 rootfs、离线 APT 依赖安装/卸载、默认 `kinakaze.cmd` 登录 shell 和退出码传递均通过。

本地候选包使用 `--preview` 标记，记录工作区状态和文件哈希；它没有替换已发布的 v0.1.0。验证覆盖上述程序与行为，不代表全部 Debian 软件均已兼容。
