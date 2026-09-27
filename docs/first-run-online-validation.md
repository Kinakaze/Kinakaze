# 首次联网启动验收（2026-09-27）

已在 Windows x86-64 的 F 盘 NTFS 目录完成一次从空 rootfs、空下载缓存开始的真实联网启动。初始化、进入登录 shell、执行工具检查和正常退出共 28.2 秒，退出码为 0。

| 项目 | 结果 |
| --- | --- |
| Debian 归档 | 302 个，107,541,414 字节，全部通过清单大小和 SHA-256 校验 |
| 安装文件 | 16,064 个，启动后逐个与清单哈希核对通过 |
| 下载源 | `https://mirrors.ustc.edu.cn/debian` 与 `https://mirrors.ustc.edu.cn/debian-security` |
| 代理 | manifest 中为 `system`，自动使用 Windows 系统代理 |
| 主机环境 | 用户进程 PATH 为空，清除通用代理环境变量，EXE 内完成下载和安装 |
| 登录环境 | `/root`，UID/GID 为 0，Bash 5.2.15 |
| 工具运行 | Python 3.11.2、Perl、APT 2.6.1、dpkg 1.21.23、nano 7.2、man 2.11.2 |
| 包管理状态 | `apt-get check` 和 `dpkg --audit` 通过 |

本次镜像地址通过发行目录中的 `rootfs.manifest.json` 配置，包路径和锁定哈希保持不变。默认 Debian 源在本机代理线路上下载缓慢且多次需要重试，因此完成验收的运行使用了上述镜像；没有用既有归档缓存替代首次下载。用户进程不会调用主机 Python，验证用 Python 驱动仅用于开发侧启动进程、记录日志和核对结果。

验收时发现并修复了普通 NTFS“修改”权限目录下的首次安装失败：Windows 启用大小写敏感目录还需要 `FILE_DELETE_CHILD` 权限。安装器现在遇到此情况时，在自己新建的暂存目录上为其所有者补齐这一项权限，子目录继承该权限；明确拒绝此权限的 ACL 仍会报错。上级发行目录的权限不变。该回归场景及已有 rootfs 测试共 24 项通过，Clippy 无警告。

目录大小写配置现在在联网下载之前完成。归档校验失败的日志也会记录实际/预期字节数和 SHA-256，便于区分不完整下载与内容不符。

本地验收目录为 `artifacts/full-first-launch-final-20260927/`：`run.json` 记录空缓存和入口文件哈希，`stdout.log` / `stderr.log` 保存完整运行输出，`validation.json` 记录文件校验结果。`Kinakaze/kinakaze.cmd` 可再次进入已经初始化的环境。
