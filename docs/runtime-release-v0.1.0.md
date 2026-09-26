# v0.1.0 运行与验证

该版本增加首次启动 rootfs 配置，以及向已有 init 进程树启动新程序的原生客户端。应用兼容性范围仍以实际行为验证为准，不代表完整 Linux ABI 或完整容器隔离。

运行包面向 Windows x86-64。宿主需要 Microsoft Visual C++ x64 运行库提供 `VCRUNTIME140.dll`；这是 init/worker 启动前的宿主依赖。包中包含完整客体基础 rootfs、Kinakaze 原生模块及 Rust 共享运行库。

## 首次启动

```powershell
.\worker.exe run -- /bin/sh
```

`worker run` 和带 root/dist 的 init 启动支持可选清单。外部 `--rootfs-manifest FILE` 优先于发行目录的 `rootfs.manifest.json`，没有清单时不执行初始化。只有选用清单且目标 rootfs 不存在或为空目录时才执行；任何非空 rootfs 都直接跳过，甚至不会读取或验证清单。清单中的相对源路径以清单所在目录为基准，目标路径以 rootfs 为基准。

清单 schema 为 1，包含 `directories` 和 `files` 数组。文件条目使用 `path` 配合 `content`，或 `path` 配合 `source`、`sha256`。例如：

```json
{
  "schema": 1,
  "directories": ["etc", "tmp"],
  "files": [{"path": "etc/hostname", "content": "kinakaze\n"}]
}
```

初始化在 rootfs 外的独占锁下执行，先验证完整计划和源文件哈希，在同级暂存目录生成文件，最后提交整个目录。成功后保留 `.kinakaze-rootfs.sha256` 记录源清单哈希；它不会用于对非空 rootfs 升级或补写。失败不向目标 rootfs 写入半成品，因此仍可按空目录规则重试。相对路径逃逸、Windows 设备名、数据流、符号链接和 reparse point 会被拒绝。已有开发发行目录没有默认清单时仍可使用；显式指定的清单缺失则报错。

发行目录中的 rootfs 已在打包时配置，可直接运行。指定其他空 rootfs 时才使用清单自动初始化。清单包含完整基础环境：所有原生模块及共享运行库、基础配置，以及从 `dependencies.lock.json` 锁定的 BusyBox、netbase 及依赖闭包。`rootfs-seed/` 保存全部文件内容，首次启动不需要 Python、联网或旧工程目录。清单只负责文件安装，不执行脚本，也不替代运行时的原生模块发现机制。默认环境不包含 Java、Minecraft 或编译开发工具。

## 向同一个 init 会话启动程序

在一个终端中启动长驻 init：

```powershell
.\init.exe --session-file .\session.json --prewarm-pool 1 --web 127.0.0.1:0
```

在其他终端中多次调用：

```powershell
$ParentPid = .\init.exe launch --session-file .\session.json -- /bin/sh -c 'while :; do /bin/sleep 1; done'
.\init.exe launch --session-file .\session.json --parent $ParentPid --wait -- /bin/sh -c 'echo $PPID; exit 23'
```

`launch` 输出新进程的逻辑 PID。省略 `--parent` 时创建当前会话的根进程；指定父 PID 时，只能插入同一 init 中仍存活的应用进程。`--cwd /path` 指定客体工作目录，重复的 `--env NAME=VALUE` 提供客体环境列表（沿用预热池的显式环境语义）。`--wait` 等待该逻辑进程最终退出，并返回其退出码。父进程在请求生效前退出、父 PID 不存在或指向预热 worker 时会报错。

启动客户端退出不结束已激活进程。长驻 init 退出后，其 Job 回收所有 worker。预热进程的 stdin 为 EOF，stdout/stderr 进入 init 的输出流；交互式 shell 使用 `worker run`。

会话文件包含启动凭据，创建时即限制为当前 Windows 用户可读写。客户端核对 init 的宿主 PID、创建时间和管道对端身份。启动凭据与 worker 凭据分开，启动客户端不能关闭 init、读取客体内存或执行 fork/exec 管理事务。会话文件应放在客体 rootfs 外。强制结束 init 后可能留下失效会话文件；确认 init 已退出后删除该文件即可重新启动，不可将文件交给其他用户。

## 构建与验证

v0.1.0 发布检查：完整 release 构建成功；1,853 项 Rust 测试通过、0 失败、27 项按现有设置忽略；43 项 Python 测试通过；29 个原生模块的 5,721 个客体导出检查通过。全工作区 Clippy 和管理 smoke 通过。运行包及解压后的 ZIP 均通过 8 项实际运行验收。

```powershell
./tools/build.ps1 -Release -TargetDirectory target/release-v0.1.0 -DistDirectory artifacts/release-v0.1.0
python tools/prepare-release-rootfs.py --dist artifacts/release-v0.1.0 --offline
python tools/test-release-runtime.py --dist artifacts/release-v0.1.0 --report artifacts/release-runtime-report.json
```

无本地依赖缓存时，移除 `--offline`，工具只下载依赖锁登记的官方包并验证哈希。发布归档的 rootfs 只含打包器配置的基础文件和原生库；验证同时覆盖内置 rootfs 和独立空 root，不包含本机账户、运行缓存、会话凭据或用户配置。

本版本使用 workspace 版本 `0.1.0`，Git 作者和提交者均为 `mitsukina`。运行归档附带依赖许可、BusyBox/netbase 对应源码和实际镜像哈希的验证报告。验证环境为 Windows x86-64，使用空 PATH 和独立临时目录；尚未在另一台全新 Windows 主机上验证。全工作区 Clippy 可运行通过，但仍有历史风格与 ABI 命名警告，不能视为零警告审计。
