# Kinakaze

**在 Windows x86-64 上运行 Linux x86-64 ELF 程序的实验性兼容层。**

[![CI](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml/badge.svg)](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#许可证)

Kinakaze 在用户态实现 Linux 程序所需的装载器、系统调用与基础库接口。运行时不需要管理员权限或自定义驱动；每个活动 Linux 进程由一个 Windows worker 承载，公共管理进程负责进程关系和生命周期。

项目处于早期开发阶段。应用兼容性取决于程序版本、依赖和运行场景，目前尚未完成完整 Linux ABI、容器隔离和独立部署验收。

## 功能与状态

- **程序装载**：x86-64 ELF、动态依赖、重定位、TLS，以及 ELF 与原生 PE 模块协作。
- **进程与文件**：逻辑 PID、fork/exec、线程、文件系统、管道、socket 与进程间通信。
- **兼容库**：libc、pthread、动态链接接口，以及持续完善中的 X11、图形、音频与 CUDA Driver API。
- **开发工具**：统一 Cargo workspace、原生模块打包、可校验的客体依赖准备工具和行为探针。

| 范围 | 已记录的验证 | 尚未完成的范围 |
| --- | --- | --- |
| 命令行与语言运行时 | BusyBox、Bash、Python、curl、Node、Java 的指定行为探针 | 任意版本和完整应用兼容性 |
| 编译器与服务 | GCC/Clang 编译运行、Redis 持久化、nginx 和 SSH 场景 | PostgreSQL 完整初始化等场景 |
| 桌面与图形 | OpenGL 探针；Minecraft 主菜单、按钮响应与正常退出 | 世界内游玩、完整桌面和音频体验 |
| 容器 | Docker 部分容器生命周期与网络探针 | 完整网络、隔离与全部容器能力 |

以上是特定构建的验证记录，不代表当前任意程序都可运行。详细环境、命令和限制见 [验证记录](docs/validation.md)、[应用专项记录](docs/README.md#兼容性与性能) 和 [开发进度](docs/progress.md)。

## 快速开始

v0.1.0 增加可选 rootfs 清单和可重新连接的 init 启动客户端。外部清单优先，仅在目标 rootfs 不存在或为空时初始化；已有非空 rootfs 保持原样。发行包内置经过哈希校验的基础 shell，启动方式及 `init launch --parent` 示例见 [v0.1.0 运行说明](docs/runtime-release-v0.1.0.md)。

### 构建环境

- Windows x86-64。
- Rust **MSVC** 工具链；当前验证使用 Rust 1.95.0。
- Visual Studio Build Tools 的 C++ 桌面开发组件与 Windows SDK。
- Python 3.11 或更新版本。
- LLVM：`clang`、`lld-link`、`ld.lld` 在 `PATH` 中可用。

使用已初始化 MSVC 环境的 PowerShell，在仓库根目录执行：

```powershell
git clone https://github.com/Kinakaze/Kinakaze.git
Set-Location Kinakaze
./tools/build.ps1 -DistDirectory artifacts/kinakaze-dist
```

构建脚本检查原生导出、格式、Rust/Python 测试，并生成 `init.exe`、`worker.exe` 与 `rootfs/lib/`。环境配置、快速编译检查及 Release 构建见 [上手指南](docs/getting-started.md)。

### 准备并运行 Linux 程序

客体程序需另行准备。提供一个 Linux x86-64 文件树，其中至少包含 `usr/bin/busybox`、`usr/bin/curl` 及其依赖；输入目录应与本仓库的输出目录分开。

```powershell
# 将此路径改为你自己的 Linux 文件树。
$GuestSource = 'C:/path/to/linux-root'

python tools/prepare-root.py --source "$GuestSource" --root artifacts/guest-root --dist artifacts/kinakaze-dist
./artifacts/kinakaze-dist/worker.exe run --root artifacts/guest-root -- /usr/bin/busybox echo hello
./artifacts/kinakaze-dist/worker.exe run --root artifacts/guest-root -- /bin/sh
```

准备工具验证 ELF 依赖闭包；缺少的已登记依赖按锁文件下载并校验。它不会自动提供完整 Linux 发行版。更多程序、离线缓存和 JRE 配置见 [客体依赖说明](tools/guest-deps/README.md)。

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `apps/` | 管理服务、worker 与命令行入口 |
| `crates/` | 公共 ABI、协议、管理、装载与宿主支持 |
| `engine/` | ELF 执行、内存、TLS、VFS 与 C 头文件 |
| `libs/` | Linux 兼容库、图形和音频模块 |
| `tools/` | 构建、打包、原生导出与客体依赖工具 |
| `tests/` | Linux 客体行为探针与运行脚本 |
| `docs/` | 上手、架构、兼容性与发布文档 |

## 文档与参与

- [文档导航](docs/README.md) · [整体设计](docs/architecture-v2.md) · [ABI 路线](docs/abi-roadmap.md)
- [贡献指南](CONTRIBUTING.md) · [报告问题](https://github.com/Kinakaze/Kinakaze/issues)
- [安全问题](SECURITY.md) · [更新记录](CHANGELOG.md) · [发布流程](docs/releasing.md)

提交问题时请提供源码提交号、Windows/Rust 版本、构建命令、最小复现和相关日志。应用运行失败应附上客体程序版本与依赖来源。

## 许可证

除另有声明的第三方内容外，Kinakaze 采用 **MIT OR Apache-2.0** 双许可证，可任选其一：

- [MIT](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

第三方源码、Rust 依赖和客体软件保留各自的许可证，详见 [第三方声明](THIRD_PARTY.md)。
