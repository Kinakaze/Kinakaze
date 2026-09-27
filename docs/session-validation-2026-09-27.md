# 2026-09-27 持久会话验收

构建目录：`artifacts/session-final/Kinakaze`。发行默认入口使用 manifest 驱动的持久进程树；默认 Debian PID 1 为 systemd，宿主 init 不占客体 PID。

实际运行结果：

- `mouse-before.json` / `mouse-after.json`：旧构建在普通 Bash 下错误打开 `1003/1006` 鼠标移动上报，ConPTY 回归稳定复现。修复默认 Win32 输入标志后，普通 Bash 不再开启鼠标上报；客体 TUI 主动启用后仍可收到 SGR 按下/释放事件；断开清理宿主模式，重连恢复客体请求，返回 Bash 后关闭鼠标上报。实现依据也见 [Windows Console 输入模式处理](https://github.com/microsoft/terminal/blob/main/src/host/getset.cpp)：关闭 Quick Edit 且保留 `ENABLE_MOUSE_INPUT` 会请求 SGR 鼠标上报。
- `mouse-sessions.json`：鼠标修复后的最终构建通过并发复用、PTY 重连、Ctrl+Z/fg/Ctrl+C、窗口缩放、退出状态、孤儿收养和回收，正常关机约 0.69 秒。
- `mouse-release-validation.json`：最终构建的 16 项发行验收通过，包含上次补充的实时下载日志验证。首次运行暴露验收脚本仅等待文件存在的竞态：shell 重定向先创建空文件再写入；测试改为等待实际完成标记，仍使用原有超时与内容断言。
- `progress-release-validation.json`：修复启动窗口日志后，16 项发行验收通过。新增用暂停中的本地下载验证 `init.exe` 和 `worker.exe` 在传输完成前显示字节进度、直接显示校验失败原因、重试不重放旧日志。`progress-rootfs-tests.log` 的 26 项安装/下载测试通过。
- `release-final-validation.json`：14 项发行验收通过，包含空 PATH、首次配置、并发启动、外部 manifest、配置保留、退出码与停止后清理。安装复用经哈希校验的 Debian 缓存；代理传输另有独立本地 HTTP/HTTPS CONNECT 验收。
- `final-systemd-sessions.json`：4 个并发客户端复用同一 init 和 Bash；Bash 与交互 ELF 脱离/重连保留 PID 和状态；Ctrl+Z/fg/Ctrl+C、40×100 终端大小、退出码 23/17、孤儿收养与 wait 回收通过。客体会话凭据确认为 root:root 0600。最终构建正常关机约 0.99 秒；此前一次运行触发配置的 15 秒兜底，最终同样回收全部进程。
- `custom-init-sessions.json`：仅改变 manifest 的 PID 1 命令、开机脚本和关机配置，同一套交互/回收测试通过。该验证配置没有运行 systemd，进程 1 是发行版提供的会话服务。自定义开机脚本生成 `CUSTOM_BOOT_OK`。
- `lifecycle.json`：托盘窗口及其实际关闭命令、WebUI 实时进程关系、强杀宿主 init 后的 Job 回收、强杀客体 PID 1 后的整树关闭、残留会话文件恢复通过。
- `apps.json`：Codex 0.157.1 和 Claude Code 2.1.283 通过新通道启动真实 TUI，操作方向键，断开后连接相同 PID。Ctrl+C 后 Codex 返回 130，Claude 返回 0。仅测试未登录界面；没有执行模型请求。Claude 使用显式客体 HTTP/HTTPS 代理。
- `unit-tests.log`：96 项 Rust 测试通过，1 项辅助入口按测试设计忽略。默认环境配置补充后，rootfs 的 9 项单元测试再次通过。相关 Python 文件编译、manifest 内服务源码一致性和 `git diff --check` 通过。

原始证据位于 `artifacts/session-20260927/`。测试使用独立 rootfs，不结束其他开发任务的进程。

通用性边界：核心仅负责进程域和会话协议；所选 PID 1 必须由发行版集成会话服务。默认 Debian 使用客体 Python 实现该服务，Windows 用户不需要 Python。rootfs 已有内容时安装器不会覆盖它，旧 rootfs 须补齐服务文件或使用新目录。默认 systemd 配置仍是 `kinakaze.target` 托管服务目标，并未宣称完整 Debian 硬件/udev/journald 开机链已全部兼容。PTY 保留有界输出历史，不提供完整屏幕快照。
