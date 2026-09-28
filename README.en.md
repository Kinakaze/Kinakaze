# Kinakaze

[简体中文](README.md) | **English**

**A user-mode compatibility layer that runs Linux x86-64 programs directly on Windows x86-64.**

[![CI](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml/badge.svg)](https://github.com/Kinakaze/Kinakaze/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Kinakaze/Kinakaze)](https://github.com/Kinakaze/Kinakaze/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Kinakaze implements the ELF loader, system calls and core libraries that Linux programs need, entirely in user mode: no virtual machine, administrator rights or kernel driver. It ships a ready-to-use Debian bookworm environment with APT and SSH, which makes it a convenient local Linux sandbox for AI agents.

> Kinakaze is at an early stage: the Linux ABI is incomplete, compatibility varies by program, and it is **not** a security isolation boundary.

## Quick start

Requirements: Windows 11 24H2 or Windows Server 2025 and later, x86-64.

1. Download `Kinakaze-<version>-windows-x86_64.zip` from [Releases](https://github.com/Kinakaze/Kinakaze/releases/latest) and extract it.
2. Double-click `init.exe`. The first launch downloads and configures the Debian base system (USTC mirror by default, following the Windows system proxy), then opens a Bash terminal.
3. Use it like any Debian system:

```sh
apt-get update
apt-get install -y gcc make jq ripgrep
```

The environment stays in the system tray: closing the terminal window only detaches. Right-click the tray icon to return to the terminal, open the WebUI or shut the environment down.

| Item | Default |
| --- | --- |
| SSH | `127.0.0.1:2222`, local connections only |
| User / password | `root` / `kinakaze` (change it with `passwd` after the first login) |
| rootfs | `rootfs/` inside the extracted directory |

Mirrors, proxies and the manifest are described in [First run and packages](docs/first-run.md); persistent sessions and systemd in [Persistent sessions](docs/persistent-sessions.md). Documentation under `docs/` is written in Chinese.

## Connecting AI agents

We recommend letting ChatGPT, Codex and similar agents use Kinakaze as a Linux sandbox over SSH or MCP: the agent runs commands, installs dependencies and executes tests inside Debian instead of changing the Windows host directly.

### Over SSH (recommended)

Set up key-based login and add an alias to `~/.ssh/config` on Windows:

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

Any agent that can run shell commands, such as [Codex CLI](https://github.com/openai/codex), can then use it directly, for example: "run and verify this inside `ssh kinakaze`".

### Over MCP

Kinakaze does not include an MCP server. Use any SSH MCP server (for example [mcp-ssh-manager](https://github.com/bvisible/mcp-ssh-manager)) with host `127.0.0.1`, port `2222`, user `root` and the key above. For Codex, register it in `~/.codex/config.toml`:

```toml
[mcp_servers.kinakaze]
command = "node"
args = ["C:/path/to/mcp-ssh-manager/src/index.js"]
env = { SSH_CONFIG_PATH = "C:/Users/<you>/.codex/ssh-config.toml" }
```

Refer to the chosen MCP server's documentation for its exact options. SSH listens on the loopback address only; before exposing it, switch to key authentication and change the default password.

## Building from source

Requires the Rust MSVC toolchain (1.95.0 verified), Visual Studio Build Tools (Desktop development with C++ and the Windows SDK), Python 3.11+ and LLVM (`clang`, `lld-link`, `ld.lld`). In a PowerShell session with the MSVC environment initialized:

```powershell
git clone https://github.com/Kinakaze/Kinakaze.git
cd Kinakaze
./tools/build.ps1 -Release -DistDirectory artifacts/kinakaze-dist
```

The build also runs format checks and tests. See the [getting started guide](docs/getting-started.md) for more options.

## Repository layout

| Directory | Contents |
| --- | --- |
| `apps/` | `init` manager service and `worker` entry point |
| `crates/` | ABI, protocol, process management, loader and host support |
| `engine/` | ELF execution, memory, TLS and VFS |
| `libs/` | libc, pthread, graphics, audio and other compatibility libraries |
| `config/` | Default rootfs manifest and locked Debian packages |
| `tools/` | Build, packaging and validation tools |
| `tests/` | Linux guest behavior tests |

## Documentation

- [First run and packages](docs/first-run.md) · [Persistent sessions](docs/persistent-sessions.md) · [WebUI](docs/webui.md)
- [Getting started](docs/getting-started.md) · [Architecture](docs/architecture-v2.md) · [Validation](docs/validation.md)
- [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Issues](https://github.com/Kinakaze/Kinakaze/issues)

## Acknowledgements

Special thanks to [OpenAI](https://openai.com) and [Codex](https://openai.com/codex) for their help in developing this project.

## License

Except for third-party content noted otherwise, Kinakaze is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Third-party licenses are listed in [THIRD_PARTY.md](THIRD_PARTY.md).
