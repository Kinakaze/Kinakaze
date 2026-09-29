# 默认启动、数据库与 io_uring 验证（2026-09-29）

Windows x86-64，Debian bookworm。原始报告保存在 `artifacts/perf-services-20260929/`。该目录中的各个构建保留各自二进制哈希；工作区存在其他并行修改，不能把整个版本间的变化全部归因于某一项优化。

最终本地测试包：`Kinakaze-0.6.0-server-test.zip`（约 23.7 MB），对应目录 `release-final/`。其 46 个原生文件与已验证的 `terminal-release/rootfs/lib` 逐文件哈希一致；SHA-256 为 `e2ce7b7809f059d54af49f4d51bbfbf6548bc5410c32431adead1ac2e995c303`。请退出旧环境后，在新目录解压启动以应用新默认配置。

## 默认启动与常驻 Python

`kinakaze.target` 默认通过 wants 链接启动 SSH、D-Bus socket 和终端会话。udev 服务、两个 socket、trigger/settle 单元增加 `/etc/kinakaze/enable-udev` 条件，默认跳过；显式创建该文件后仍能启动并测试。journald 和 tmpfiles 等普通服务依赖保持按需可用。

默认 `python3` 运行 `/usr/lib/kinakaze/session.py`，承担 PTY、终端断开重连、信号与子进程回收。它使用客体自带 Python，不要求宿主安装 Python。本轮没有删除这个服务。

会话唤醒改用 Unix socketpair，并让 `poll` 的只读 PTY 注册使用原生就绪事件；该事件也进入混合 TCP/PTY 的 AFD 等待。只更换 socketpair 的中间版本没有可靠 CPU 收益，记录在 `session-idle-owned/`。最终混合等待探针的 poll/epoll 两种 250 ms 空闲等待均测得 0 ms 进程 CPU，且数据、重置及挂断通过，见 `terminal-feature-final/pty-wait-*`。这受宿主 CPU 计时分辨率限制，不表示任意负载下绝对零开销。

`terminal-idle-final/` 在同一 `terminal-release` 构建、同一根目录上按旧管道/新 socket/新 socket/旧管道运行，每轮 15 秒、分三段采样，四轮启动和关闭均通过。旧脚本取自提交 `1f79cad`，Python 单核 CPU 中位数约 **1.72%**；新脚本两轮的六段采样均为 **0%**，处于计时精度以内。统计使用测试拥有的 Windows Job，包含 exec 后 Windows 父进程已经退出的 worker，避免用 PPID 树漏掉 Python。报告保留逐进程 CPU 与工作集；100% 表示占满一个逻辑 CPU，工作集不等于独占物理内存。systemd 本身仍有非零空闲消耗。

systemd 的 signalfd 映射，以及 inotify 的原生与内存文件系统监听映射在单次等待内复用；仍读取共享发布状态，观察跨 worker 修改。`comparison-completed/` 的两轮旧构建空闲样本约 8.7%–10.0% 单核，新构建约 5.6%–10.0%；变化存在噪声，不能宣称消除了 systemd 轮询。

## 标准 io_uring 与内存策略

之前的 Windows IoRing 后端只有自定义提交接口，标准 liburing 使用 syscall 425 会得到 ENOSYS。本轮接通 Linux `io_uring_setup/enter/register` 和 SQ/CQ/SQE mmap ABI，完成队列由原生 I/O 完成事件驱动发布。

已实现并测试的范围：

- SQ/CQ 索引、SINGLE_MMAP、CQSIZE/CLAMP、异步 CQ 发布、CQ 满后的完成项保留与继续发布。
- NOP、READ/WRITE、READV/WRITEV、FSYNC/FDATASYNC、按 user_data 取消及取消/完成竞态。
- 固定文件注册、更新、注销，原 fd 关闭后仍保留注册的文件对象；eventfd 注册和完成通知。
- 能力探测、错误 SQE/CQE、无效地址、跨线程等待、临时信号掩码、poll/epoll 就绪。
- `MADV_DONTDUMP`/`MADV_DODUMP` 使用 Windows Error Reporting 的真实排除接口，维护部分区间、重叠、重新映射、unmap 与 fork 后的进程状态。
- `MADV_DONTFORK`/`MADV_DOFORK`、`MADV_WIPEONFORK`/`MADV_KEEPONFORK` 使用实际 fork 映射策略；验证子进程中省略区间、清零、第二代 fork、父进程内容保留和恢复默认策略。
- `mincore` 不再无条件返回所有页驻留，改为校验有效映射并查询本进程原生工作集。

**这不是 Linux io_uring 的全量实现。** SQPOLL/IOPOLL、固定缓冲区、linked/multishot 操作、套接字操作及 ring 的跨 worker 传递/继承等仍不支持，不通过伪造成功或伪造 probe 标志隐藏缺口。WER 排除不阻止调试器主动读取进程内存，也不等于已经实现 Linux core 文件生成器；`mincore` 的工作集查询不代表 Linux 全局文件页缓存统计。

## 应用与安装结果

| 对象 | 已验证行为 | 报告 |
| --- | --- | --- |
| MariaDB 10.11.18 | InnoDB 提交/回滚、32 次并发客户端写入、正常重启、SIGKILL 后恢复持久数据；三次启动均使用 io_uring，无 DONTDUMP/DONTFORK 告警 | `fresh-apps-final/results.json` |
| PostgreSQL 15 | 非特权初始化、事务、并发查询、正常重启后数据 | 同上 |
| Redis 7 | 实际命令、事务、Lua、二进制数据、持久化与重启 | 同上 |
| SQLite | 不同进程的 rollback/WAL 读写、锁冲突、Unicode 路径 | 同上 |
| Nginx | 主从进程、HTTP、文件/Range/HEAD、并发请求、reload 与退出 | 同上 |
| FFmpeg | 音频文件与管道、FLAC、重采样、FFV1 图像像素往返 | 同上 |
| OpenJDK 17 | JIT、线程、文件和 ProcessBuilder 子进程 | 同上 |
| AstrBot 4.28.1 | Linux Python 3.12.14、实际后台启动、登录、两轮各 16 次并发 API 请求、SQLite 数据库创建及重启 | `fresh-astrbot-final/results.json` |

AstrBot 使用上游源码 `b53999e959cfc3b71d7b74713ddee837be843fcb`，Linux 依赖安装在独立测试根的 `/opt/astrbot-deps`。测试未接入外部聊天账号或在线模型，不把后台 API 验证等同于所有平台消息收发成功。第一轮启动包含 WebUI 资源下载，不能当作纯程序启动基准。

最终验收重新创建 `final-root/`，通过真实 APT 安装上述服务器、媒体工具和 JDK，`apt-get check` 与 `dpkg --audit` 通过。九项应用/基础行为测试全部通过，四项服务器特性探针各重复两轮全部通过（`fresh-features-final/`）。新根上的 AstrBot 首次就绪约 110.4 秒，包含资源下载和冷加载；同一数据目录重启就绪约 12.3 秒。此前缓存已准备的根启动更快，不能将两种条件混在一起比较。

真实安装还发现并修复了基础包缺少 `Multi-Arch: allowed`、丢失上游虚拟 Provides、ucf 的 debconf 模板路径，以及只写句柄覆盖只读文件时 fstat 权限错误。`first-run-final.json` 验证新根安装和带 `python3:any`、`perl:any`、`perlapi-5.36.0` 依赖的 APT 安装/卸载。已有测试根的 `dpkg --configure -a`、`apt-get check`、`dpkg --audit` 通过。

另外补齐了 MariaDB 所需的浮点异常屏蔽接口，并修复 pthread 新线程没有继承信号掩码的问题。否则 SIGTERM 可能被错误线程的告警处理器消费，导致数据库停止超时。

## 回归与复现

`server-release-build.log` 中 2015 项 Rust 测试通过、29 项既有忽略；原生导出与 Python 工具检查通过。后续 PTY 等待改动由 `terminal-feature-final/` 的实际客体回归补充。`shared-watches-final/` 覆盖 fork/exec、SCM_RIGHTS、跨进程监听修改、最后关闭和创建者异常退出。`sessions-final-retry/results.json` 验证并发复用、终端重连、窗口尺寸、作业控制、孤儿回收、退出状态与整树关机。

最终 PTY 修改后同一套会话测试再次通过，见 `terminal-sessions-final/results.json`。`systemd-budgeted-final/results.json` 的 46 项服务检查全部通过，包含默认 udev 跳过与显式启用、journal 写读/轮转、定时器、路径激活、凭据、socket/fdstore、服务重启和 daemon-reexec。

```powershell
python tools/test-server-features.py --root <独立测试根> --dist <运行库目录> --output <报告目录> --repeat 3
python tools/test-common-workloads.py --root <独立测试根> --dist <运行库目录> --output <报告目录> --only '^(mariadb|postgresql|redis|sqlite-processes|nginx|ffmpeg|java)$'
python tools/test-astrbot.py --root <已准备 AstrBot 的测试根> --dist <运行库目录> --output <报告目录>
python tools/test-service-manager.py --root <独立测试根> --dist <运行库目录> --output <报告目录> --default-boot
```

完整 unit 文件枚举仍然较慢。`unit-list-profile/` 中同一输出的在线全量枚举约 13.1 秒，离线扫描约 18.2 秒，带 `*.socket` 模式的在线枚举约 1.0 秒。生命周期测试累积更多 unit 状态后，一次全量枚举耗时约 71.6 秒。超过默认 25 秒 D-Bus 超时和原整套 180 秒预算的失败记录均保留；功能回归现给这条全量操作 90 秒总线预算、整套 275 秒，并继续记录实际耗时。这是剩余性能问题，不将增加测试预算记为运行时优化。
