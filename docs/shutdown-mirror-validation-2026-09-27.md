# 关闭响应与国内默认源验收

默认 Debian manifest 改用 `https://mirrors.ustc.edu.cn/debian` 和 `https://mirrors.ustc.edu.cn/debian-security`，同时更新首次安装下载设置和客体 APT 配置。镜像和代理仍可由 manifest / 环境变量覆盖；保留自动跟随系统代理的默认行为。核心没有新增发行版或 init 类型判断。中科大提供对应的 [Debian](https://mirrors.ustc.edu.cn/help/debian.html) 和 [安全更新](https://mirrors.ustc.edu.cn/help/debian-security.html) 仓库。

本机实际验证（证据位于 `artifacts/session-shutdown-mirror/`）：

- `mirror-packages.json`：禁用代理，对 306 个锁定包逐项检查 HTTPS 可访问性和长度，全部通过。
- `direct-install.log`：原生 `worker.exe setup` 使用空下载缓存和 `KINAKAZE_DOWNLOAD_PROXY=direct`，下载并按清单校验全部 306 个包，安装 16101 个文件成功。
- `direct-boot.json` / `apt-update.log`：新安装的 Debian 启动成功，APT 显式禁用 HTTP/HTTPS 代理后从新默认源获取主仓库、updates 和 security 索引，退出码为 0，无下载失败警告。测试 SSH 使用临时端口，发行默认仍为 127.0.0.1:2222。
- `shutdown.json`：通过真实托盘关闭命令，正常关闭终端耗时约 11 ms、整树退出约 0.72 s。模拟 init 不退出和会话服务不响应时，终端分别在约 10/21 ms 内退出，完整进程树在约 3.21/3.16 s 内回收；记录的所有宿主 worker 均已退出。
- `release-validation.json`：16 项发行验收通过，包含镜像/代理覆盖、安装进度、首次安装、配置保留、持久会话复用与整树清理。

关闭时立刻通知终端并更新托盘状态；配置的正常关机请求在后台执行，独立期限不会被控制通道的写入或回复等待拖长。默认 Debian 的退出期限由 manifest 从 15 秒改为 3 秒，到期开始 Job 回收并等待成员归零。高负载下也可能使用这一兜底路径；测量值不是所有机器上的固定时长。

已有非空 rootfs 不会被安装器覆盖。替换发行文件后，新的退出期限在环境重启时生效；已有 APT 配置可按 [首次安装文档](first-run.md#下载镜像与代理) 的命令将旧默认地址替换为新镜像。
