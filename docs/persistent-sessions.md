# 持久运行环境、托盘与启动配置

每个规范化 rootfs 路径只允许一个常驻 `init.exe`。它管理 Windows Job、控制通道、托盘和 WebUI，不占客体 PID。客体唯一的 PID 1 是 manifest 的 `startup.command` 指定的程序，由 `worker.exe` 承载。核心运行时不判断 init 名称，不绑定 systemd。

默认 Debian manifest 使用 systemd 和 `kinakaze-session.service`。会话服务拥有 PTY，通过真实 `fork → setsid → TIOCSCTTY → execve` 创建 Bash、裸 ELF、Codex 或其他程序。Bash 内的命令仍由 Bash 创建和等待，外部启动不再指定任意 `--parent`。

## 使用

双击 `init.exe`，或运行 `worker.exe`。首次启动安装 rootfs，后续连接同一环境。Windows 不需要 Python：EXE 实现原生入口、托盘和终端客户端；默认会话服务使用随 Debian rootfs 下载的 Python。

```powershell
.\worker.exe                        # 回到默认终端
.\worker.exe run -- /usr/bin/my-elf  # 同一环境的新终端，自动分配名称
.\worker.exe run --name work -- /bin/bash -l
.\worker.exe session attach --name work
.\worker.exe session status
.\worker.exe session stop
```

命令接受 `--root ROOT`、`--dist DIST`；启动接受 `--rootfs-manifest FILE`。修改开机配置后，停止再启动才生效；连接存活环境不重复执行开机脚本。

`Ctrl+]` 或关闭窗口只断开客户端，保留 PID、环境变量、工作目录、PTY 和最近 256 KiB 输出。再次连接重放有界历史并继续交互；这不是完整终端屏幕快照。每个 PTY 同时只有一个输入客户端，新连接接管旧连接。终端大小同步到客体，断开期间持续排空输出，避免程序因客户端离开而堵塞。

普通 shell 默认不启用鼠标上报；TUI 可通过标准终端序列主动启用。连接前和正常断开时清理宿主输入模式，避免鼠标坐标串写进命令行。重连时由客体输出恢复请求的模式。

托盘右键动态列出已启动终端，另有“打开 WebUI”和“关闭整个进程树”。已退出的命名终端保留退出结果，连接不会偷偷创建第二个程序；新进程使用新名称或不指定名称。

## Manifest

```json
"startup": {
  "command": ["/usr/local/sbin/kinakaze-systemd"],
  "script": null,
  "shutdown": ["/bin/systemctl", "--no-block", "exit"],
  "session_config": "/etc/kinakaze/session.json",
  "environment": {"HOME": "/root", "USER": "root", "LANG": "C.UTF-8"},
  "terminals": [{
    "name": "bash", "command": ["/bin/bash", "-l"], "cwd": "/root",
    "environment": {"TERM": "xterm-256color"}, "autostart": true
  }],
  "default_terminal": "bash",
  "tray": true,
  "web": "127.0.0.1:0",
  "shutdown_timeout_seconds": 3
}
```

`command` 创建 PID 1；脚本入口应 `exec` 到所选 init，保留 PID 1。`script` 是会话服务启动终端前执行并等待的可选 argv 数组，例如 `["/bin/sh", "/etc/my-startup.sh"]`。失败不发布可连接环境。`shutdown` 是发行版关机命令；未配置时向 PID 1 发送 SIGTERM，会话服务自身是 PID 1 时直接进入回收流程。

`startup.environment` 提供开机脚本和应用的默认环境，终端自己的 environment 可覆盖它。HOME、PATH、账号等由发行版配置。`terminals` 统一使用 argv、cwd、environment、autostart，没有按应用种类分支。`autostart:false` 可通过 `worker session start --name NAME` 启动。同名配置相同才复用，冲突报错，不覆盖运行中的命令。

替换 systemd 时，发行版开机脚本负责启动会话服务。例如非 systemd 验证配置把 `command` 改为 `["/usr/bin/python3", "-u", "/usr/lib/kinakaze/session.py", "/etc/kinakaze/session.json"]`、`shutdown` 改为 `[]`；服务直接担当 PID 1，宿主和终端代码不变。发行版也可提供不依赖 Python 的服务实现。

`session_config` 指定临时连接配置路径，内容包括 loopback 端口、一次性凭据和 startup 数据。服务启动命令必须接收该路径。默认 unit、服务脚本和启动软链接都是 manifest 数据。已有非空 rootfs 不会被首次安装器覆盖；升级旧 rootfs 须安装新增服务文件与启动集成，或使用新 rootfs 目录。

## 生命周期

会话服务设为 subreaper，收养其退出父进程留下的后代并执行 `waitpid`。其他孤儿由客体注册表按最近祖先 subreaper/init 规则收养。宿主观察退出码不消费客体父进程的等待结果。客户端不是客体父进程，客户端退出不触发 reparent。

关闭后立即通知交互客户端退出，托盘显示“正在关闭进程树”；后台先执行配置的关机命令。默认 Debian manifest 给出 3 秒正常退出期限，较慢的服务可修改 `shutdown_timeout_seconds`。期限独立于控制通道的发送和回复等待；超时通过本环境 Windows Job 回收全部 worker，等待成员归零再释放启动锁。PID 1 真正退出也关闭整树；exec 退役的旧 worker 不会误触发关闭。强制结束宿主 init 时，Job 的 kill-on-close 回收其所属 worker。

宿主向已连接的终端发送 `{"event":"shutdown"}` 后关闭该客户端连接，客体关机继续在后台进行；状态查询的 `closing` 标记和启动锁保持有效，直至整树回收结束。

宿主锁、凭据和 `init.log` 位于 rootfs 同级 `.kinakaze-session-*` 目录；客体启动日志为 rootfs 内 `.kinakaze-boot.log`。凭据文件使用当前 Windows 用户专属 ACL，客体会话配置权限为 root:root 0600。WebUI 仅监听 loopback，保留同源/CSRF 校验；实时父子关系取自客体 `/proc`，结束操作发送客体信号。

## 服务接口与验证

服务读取配置的 `port`、`token`、`startup`，连接 loopback TCP，发送 `{"role":"broker","token":"…"}`。协议为每行一个 JSON，单帧最多 1 MiB。请求含 `id/op`，返回 `{"id":id,"ok":result}` 或 `{"id":id,"error":"message"}`。操作包括 start/input/resize/signal/processes/shutdown。异步事件包括 terminal（name/pid/ppid/status）、ready、output（十六进制 data）、exit（负状态表示信号）与 closed（PTY EOF）；processes 返回 pid/ppid/program 列表。参考实现：`config/session.py`。

验收：`tests/guest/run-sessions.py`，覆盖并发启动、PID 1、PTY 重连、变量保留、Ctrl+Z/fg/Ctrl+C、裸 ELF 交互、退出码、孤儿收养/回收与关机。开发验收使用 pywinpty，发行包不需要宿主 Python。

原有 `init --session-file`、预热池和 `worker oneshot` 保留为开发验收接口；默认发行入口使用持久环境。此处验证的生命周期语义不代表已实现 Linux 全部系统调用或完整 Debian 开机服务链。
