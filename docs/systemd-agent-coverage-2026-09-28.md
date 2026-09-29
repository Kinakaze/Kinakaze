# 事件等待、systemd 与开发工具进展

## 目标与所有权

继续现有目标：跨 worker 全局状态和需要保活的内核对象由 init 托管；建立常用命令、systemd、Clang 和实际 agent 工具流程的回归与性能对照。目标尚未完成。

审查了未接入的 `unix/remote_fd.rs`：`fetch` 在临时 `Queue` 上 push 后立即 drop，队列的命名 section/mutex 没有可靠持有者，可能在目标读取前消失。该文件以及 pidfd 文件当前没有接入模块，不能计为已支持的能力；完整实现还需要 init 托管、取消与回收协议的审查和测试。

## 事件驱动变更

- Unix stream/listener 的只读、非 ET epoll 注册在已有完整原生通知源时取消 10 ms 周期回扫。数据、连接和对端关闭由既有通知源唤醒。混合 AFD/Unix 集合使用已有 fan-in。其他兴趣、消息 socket、ET 等仍保留回退，避免未经验证地移除必要通知路径。
- 单次 epoll 等待内复用 `/proc/*/mountinfo` 的共享映射和版本快照。每次检查共享版本，读取确认后刷新快照；权威数据仍在 init 托管的共享对象中。注册消失时清理视图，等待结束释放映射，不引入 worker 持有的权威全局状态。
- `tests/guest/UnixEpollWakeProbe.py`：纯 Unix 和混合集合的空闲超时、数据到达、强杀对端后的 EOF/唤醒通过。300 ms 空闲测得进程 CPU 时间为 0，受计时精度限制。
- `tests/guest/MountPollProbe.py`：跨 worker 挂载唤醒、共享描述符读取确认、卸载再次通知通过。
- 28 项 epoll 测试和 7 项 Unix readiness 测试通过，覆盖取消、关闭/复用、信号、大集合和原生等待源生命周期。

产物：`artifacts/goal-systemd-candidate`。回归记录：`artifacts/goal-unix-epoll-wake`、`artifacts/goal-mount-poll`。

最终候选的 `artifacts/goal-event-compatibility/report.json` 离线回归通过，包含 proc、文件系统边界、Bash 中断和确定性编辑样例；不算在线模型回归。

## systemd 空闲 CPU

`tools/test-service-manager.py` 新增 `--idle-seconds` 和可选的 `--idle-processes`（需要 psutil）。先等待 guest 探针退出和池补充，再以最长 5 秒窗口采样 Windows Job 的 CPU/IO；可按原生进程分解。100% 表示占满一个核心。就绪失败后的诊断采样明确记录 `idle_after_readiness_passed=false`，不冒充正常启动验收。

隔离根目录 `artifacts/goal-systemd-isolated/rootfs` 用完整、哈希校验的 Debian manifest 重新安装，不复制使用中的根目录。另一个独立开发根用于 APT/Clang/pi。

- `artifacts/goal-systemd-idle/minimal/results.json`：最小目标启动、PID 1、machine-id 与关机通过，空闲约 18.4%–20.9% 单核。
- 原生进程归因显示消耗集中在 systemd 所在 worker，init 和备用 worker 接近零。
- 仅复用 mountinfo 映射的候选 `artifacts/goal-systemd-isolated/candidate/results.json`：启动/关机通过，空闲仍约 16.9%–19.1%。根目录、首次启动状态不同，因此不将这个差异作为可靠性能收益；主要高 CPU 问题未解决。
- 默认服务目标的 SSH 服务没有激活，完整默认启动验收失败，记录保留在 `artifacts/goal-systemd-idle/attribution`。

下一步需补齐 timerfd、signalfd、inotify 和挂载变化的事件/截止时间通知，验证重置、重装、取消、跨 worker 传递和持有者死亡。不能通过拉长轮询间隔宣称解决。

## pi 和 Clang

Node 22.23.3 从官方发行包下载并校验 SHASUMS256；pi 0.73.1 安装成功，版本与帮助命令通过。该版本来自 `@mariozechner/pi-coding-agent`，包发布者提示新版本迁移至 `@earendil-works/pi-coding-agent`，后续可再覆盖新包。

`tools/test-pi-workflow.py` 使用真实 pi CLI、本地确定性 Anthropic SSE 服务和真实 guest 文件/子进程，验证 read → bash 失败测试 → edit → bash 成功测试。核对每个 tool_result、错误状态、最终源码和未修改的三项测试。`artifacts/goal-pi-workflow-verified/report.json` 为通过；这不是在线模型能力评测。

Clang 14 已安装并成功编译 33 个翻译单元。首次链接暴露开发环境缺少 ELF 导入库的问题：Linux ld 不能直接链接 Windows PE provider。使用构建已有 `elf-imports` 并显式设置 `-L`/`-rpath-link` 后，链接和执行通过。`tests/guest/ClangBuildProbe.py` 覆盖两次干净构建、无变更、单源文件和共享头增量，验证编译数量及输出。完整结果以 `artifacts/goal-clang-verified` 为准，尚无可信的 Clang 优化前后收益结论。

最终五项构建均通过：两次干净构建 53.11/53.60 秒（各 33 个单元），无改动 0.264 秒（0 个），单源文件 4.26 秒（1 个），共享头 53.68 秒（32 个，main 未包含该头）。这是开发构建的一次场景基线，不能外推为 release 性能或相对旧版的加速。

APT 安装开发工具发现 `kinakaze-base` 与 Perl/Python 的真实包文件冲突。在独立开发根中使用 force-overwrite 后，Python 的 rtupdate hook 仍因 `apt-listchanges` 无独立 dpkg 包记录而失败，继而影响 gyp/npm 配置。未把这些安装计为正常成功；pi 使用独立官方 Node 包自带的 npm。包所有权和 maintainer-script 兼容仍需修复。

## 单次等待的 timerfd / inotify 映射复用

新候选 `artifacts/goal-wait-views-candidate` 在单次 epoll 等待内保留 timerfd 共享页和 inotify Store/目录队列的映射，避免每次 10 ms 回扫都重新打开、映射和关闭。没有缓存计时器值或 watch 表：每轮仍在共享锁下读取当前状态。目录 I/O 仍由 init 持有；缓存按 watch 编号和对象 ID 校验，移除 watch 后回收，等待返回后全部释放。清理避免对每个缓存项扫描完整 watch 表。

这一步减少回退路径开销，**没有补齐 timerfd/inotify 的事件通知，也没有取消其轮询**。完整事件路径仍需解决跨 worker 重装、read 确认、clock-change 和 watch 集合变更的通知，不能只把轮询间隔调大。

验证记录：

- 最终修订 37 项单测通过：timerfd 3、inotify 6、epoll 28。包含别名重装/解除、读取清空、原始 fd 关闭、watch 新增/移除及缓存回收。
- `tests/guest/SharedWaitViewProbe.py` / `artifacts/goal-shared-wait-views/result.json`：父进程已进入 epoll 等待后，子 worker 重装定时器并退出、跨 worker 新增/删除 watch、别名读取确认，全部通过。
- `artifacts/goal-pi-wait-views/report.json`：真实 pi CLI 的确定性 read/bash/edit 流程通过（5 次本地 API 请求），不是在线模型评测。
- `artifacts/goal-wait-views-compatibility/report.json`：proc、文件系统边界、Bash 中断及离线编辑回归通过。
- 构建、格式、Python 语法和 diff 空白检查通过。

同一个隔离根连续按 baseline/candidate/candidate/baseline 测量，每轮 15 秒分三段采样，期间不运行其他构建或回归。`artifacts/goal-systemd-wait-views-comparison` 中四轮启动/关机均通过；单核 CPU 百分比的轮内中位数依次为 **18.12、18.75、20.31、18.12**。候选未显示 systemd CPU 收益，样本反而略高；不能宣称已解决 systemd 空闲占用。

`tests/guest/WaitSetIdleProbe.py` 的独立 16 描述符空闲等待（每次 750 ms、各三次）在 `artifacts/goal-wait-views-idle` 留档。timerfd 的进程 CPU 中位数为 baseline 78.125 ms / candidate 62.5 ms；inotify 为 93.75 ms / 46.875 ms。样本少且 CPU 计时粒度约 15.625 ms，只能作为这两条路径开销下降的初步证据，不能外推 systemd 或发布版收益。

两次开启全局诊断的 systemd 探针保留在 `artifacts/goal-systemd-wait-trace` 和 `artifacts/goal-systemd-wait-categories`。诊断文本进入 systemctl 的合并输出，使其不再精确等于 `active`，故就绪验收失败；这些记录不计入上述性能比较。后续归因应只针对 PID 1 并使用独立诊断输出通道。
