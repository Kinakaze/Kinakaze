# Kinakaze

**简体中文** | [English](README.en.md)

**在 Windows x86-64 上直接运行 Linux x86-64 程序的用户态兼容层。**

[![CI](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml/badge.svg)](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Kinakaze/Kinakaze)](https://github.com/Kinakaze/Kinakaze/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#许可证)

Kinakaze 在用户态实现 Linux 程序所需的 ELF 装载器、系统调用和基础库，不需要虚拟机、管理员权限或内核驱动。开箱提供一个 Debian bookworm 环境，可用 APT 安装软件、通过 SSH 登录，适合作为本机的 Linux 沙盒交给 AI agent 使用。

> 项目处于早期阶段：尚未实现完整的 Linux ABI，兼容性因程序而异，也**不是**安全隔离边界。

## 快速开始

系统要求：Windows 11 24H2 或 Windows Server 2025 及以上，x86-64。

1. 从 [Releases](https://github.com/Kinakaze/Kinakaze/releases/latest) 下载 `Kinakaze-<版本>-windows-x86_64.zip` 并解压。
2. 双击 `init.exe`。首次启动会自动下载并配置 Debian 基础环境（默认使用中科大镜像，自动跟随系统代理），完成后打开 Bash 终端。
3. 之后可以像普通 Debian 一样使用：

```sh
apt-get update
apt-get install -y gcc make jq ripgrep
```

环境常驻在系统托盘：关闭终端窗口只会断开连接，右键托盘图标可以回到终端、打开 WebUI 或关闭整个环境。

| 项目 | 默认值 |
| --- | --- |
| SSH | `127.0.0.1:2222`，仅本机可访问 |
| 账号 / 密码 | `root` / `kinakaze`（首次登录后请用 `passwd` 修改） |
| rootfs | 解压目录下的 `rootfs/` |

镜像、代理和清单配置见 [首次安装与包管理](docs/first-run.md)，常驻会话与 systemd 见 [持久会话配置](docs/persistent-sessions.md)。

## 接入 AI Agent

推荐让 ChatGPT / Codex 等 agent 通过 SSH 或 MCP 使用 Kinakaze 作为 Linux 沙盒：agent 在 Debian 里执行命令、安装依赖和运行测试，不直接改动 Windows 宿主环境。

### 通过 SSH（推荐）

先配置免密登录，并在 Windows 的 `~/.ssh/config` 中添加别名：

```powershell
ssh-keygen -t ed25519 -f $HOME\.ssh\kinakaze
Get-Content $HOME\.ssh\kinakaze.pub | ssh -p 2222 root@127.0.0.1 "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys"
```

```sshconfig
Host kinakaze
    HostName 127.0.0.1
    Port 2222
    User root
    IdentityFile ~/.ssh/kinakaze
```

之后能执行 shell 命令的 agent（如 [Codex CLI](https://github.com/openai/codex)）都可以直接使用，例如告诉它：“在 `ssh kinakaze` 中运行并验证”。

### 通过 MCP

Kinakaze 不内置 MCP 服务。可以使用任意 SSH 类 MCP 服务（例如 [mcp-ssh-manager](https://github.com/bvisible/mcp-ssh-manager)），把目标设为 `127.0.0.1:2222`、用户 `root` 和上面的密钥。以 Codex 为例，在 `~/.codex/config.toml` 中登记：

```toml
[mcp_servers.kinakaze]
command = "node"
args = ["C:/path/to/mcp-ssh-manager/src/index.js"]
env = { SSH_CONFIG_PATH = "C:/Users/<you>/.codex/ssh-config.toml" }
```

具体参数以所选 MCP 服务的文档为准。SSH 只监听本机地址；如需对外开放，请先改用密钥认证并修改默认密码。

## 从源码构建

需要 Rust MSVC 工具链（已验证 1.95.0）、Visual Studio Build Tools（C++ 桌面开发与 Windows SDK）、Python 3.11+ 和 LLVM（`clang`、`lld-link`、`ld.lld`）。在已初始化 MSVC 环境的 PowerShell 中执行：

```powershell
git clone https://github.com/Kinakaze/Kinakaze.git
cd Kinakaze
./tools/build.ps1 -Release -DistDirectory artifacts/kinakaze-dist
```

构建会同时运行格式检查和测试。更多选项见 [上手指南](docs/getting-started.md)。

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `apps/` | `init` 管理服务与 `worker` 入口 |
| `crates/` | ABI、协议、进程管理、装载与宿主支持 |
| `engine/` | ELF 执行、内存、TLS 与 VFS |
| `libs/` | libc、pthread 及图形、音频等兼容库 |
| `config/` | 默认 rootfs 清单与 Debian 软件包锁定 |
| `tools/` | 构建、打包与验证工具 |
| `tests/` | Linux 客体行为测试 |

## 文档

- [首次安装与包管理](docs/first-run.md) · [持久会话配置](docs/persistent-sessions.md) · [WebUI](docs/webui.md)
- [上手指南](docs/getting-started.md) · [整体设计](docs/architecture-v2.md) · [验证记录](docs/validation.md)
- [更新记录](CHANGELOG.md) · [贡献指南](CONTRIBUTING.md) · [安全策略](SECURITY.md) · [问题反馈](https://github.com/Kinakaze/Kinakaze/issues)

## 致谢

特别感谢 [OpenAI](https://openai.com) 与 [Codex](https://openai.com/codex) 对本项目开发的帮助。

## 许可证

除另有声明的第三方内容外，Kinakaze 采用 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 双许可证，可任选其一。第三方组件的许可证见 [第三方声明](THIRD_PARTY.md)。
