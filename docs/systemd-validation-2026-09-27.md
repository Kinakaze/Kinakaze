# systemd 与默认 SSH 验证（2026-09-27）

Windows x86-64 上运行 Debian systemd 252.39，使用实际 PID 1、原生
`systemctl` 和 OpenSSH 客户端。

## 默认发行行为

`kinakaze-systemd.cmd` 调用 release 的 `worker.exe`。首次使用由 EXE
读取默认清单，安装锁定的 306 个 Debian 软件包，用户无需主机 Python。
启动脚本先生成并保存 machine-id，然后以 PID 1 启动 `kinakaze.target`。
默认 SSH 仅监听 `127.0.0.1:2222`，账号 `root`，密码 `kinakaze`。
首次启动生成独立主机密钥；日志写入 `/var/log/sshd.log`。

默认目标及 SSH 使用 `DefaultDependencies=no`。运行时负责提供进程、
挂载和网络环境，因此此配置不启动 Debian 的完整硬件、内核与日志服务链。
其他应用服务可采用同样的依赖配置并将输出写入文件。原样启动
`multi-user.target`、journald、udev，以及完整 cgroup 控制、驱动和全部
Linux 隔离机制，均不属于此次通过的兼容范围。

## 修复

- 修复 fork 后 libc 程序名全局变量、GNU printf `%m`、signalfd/SIGCHLD
  投递，以及默认忽略信号对 Unix socket 等待的错误中断。
- 实现命名 Unix datagram 的共享消息队列、源地址和凭据；补全 tmpfs
  socket inode、inotify、chroot，使服务通知和 SSH 权限分离能够运行。
- 修复 timerfd 极远截止时间溢出，接入 Windows 时钟变更取消通知。
- 实现可轮询的 proc 挂载表及嵌套 epoll，拒绝循环监听；补全 systemd
  使用的 netlink 选项。修复 chmod 后已有写描述符的 ftruncate 权限。
- 区分 GNU `basename` 和 POSIX `__xpg_basename`：GNU 入口返回原输入中的
  指针，避免共享缓冲区破坏 systemd 的 unit 别名和 preset 比较。
  语义依据 [GNU libc 文档](https://sourceware.org/glibc/manual/latest/html_mono/libc.html)。

## 已执行的验证

- 全新 rootfs 启动：主机 PATH 为空；无预置 SSH 主机密钥；持久 machine-id。
- `systemctl is-system-running` 为 `running`，失败 unit 和待执行 job 均为 0。
- 实际 `Type=notify` 服务发送 `READY=1`，完成启动、状态查询和停止。
- Windows OpenSSH 正确密码登录、错误密码拒绝、远程退出码传递、真实
  ConPTY 交互终端，以及 SSH 服务 restart/stop/start 后的端口与登录检查。
- `SystemdRuntimeProbe.py` 验证路径函数、信号、时钟、netlink、挂载通知、
  嵌套 epoll 循环拒绝、fork/exec 共享状态、tmpfs socket 与 chroot。
- 56 项 timerfd、inotify、signal、Unix socket 单元测试，22 项 Python 工具测试。
- 12 项发行验收，包括空 PATH、默认启动、并发安装、含空格路径、
  外部 manifest、代理与环境变量覆盖、保留已有配置及进程生命周期。

Codex 0.157.1 与 Claude Code 2.1.283 另外通过真实 ConPTY 检查启动菜单、
回车、方向键和 Ctrl+C。Claude 使用显式 guest HTTP/HTTPS 代理；本机直连
Anthropic 返回 HTTP 403。测试停留在登录界面，不代表已完成账户登录或在线推理。

## 复现与证据

运行库目录：`artifacts/systemd-online-validated/`。证据位于
`artifacts/systemd-20260927/`，包含 `ssh-final.json`、`release-validation.json`、
`runtime-probe-r13.log`、`unit-results.json`、`python-tests.log` 和终端记录。
首次安装回归复用了经过 SHA-256 校验的 Debian 缓存；独立联网安装记录
见 [首次联网验收](first-run-online-validation.md)。

```powershell
$env:PYTHONPATH=(Resolve-Path artifacts/promo-v1/python).Path
python tests/guest/run-systemd.py --dist artifacts/systemd-online-validated --root <测试 rootfs> --report <报告.json>
python tools/test-release-runtime.py --dist artifacts/systemd-online-validated --report <发行报告.json>
```

以上 Python/pywinpty 只供开发者验收使用，不放入用户运行包。
