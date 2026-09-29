# 日常命令性能与资源生命周期进展

## 可复现测量

`tools/benchmark-daily-commands.py` 使用持续运行的 init 和新 worker，执行进程启动、文本处理、文件树复制、压缩解压、进程查询、包查询、Python 文件处理七类工作流。每个样本有唯一目录和完成标记，验证命令结果及退出状态。记录构建哈希、wall time、进程树 CPU/IO、预热与正式样本、中位数和 p95。p95 使用 nearest-rank，三次正式样本时就是最大值，不代表稳定尾延迟。

```powershell
python tools/benchmark-daily-commands.py --root artifacts/full-first-launch-final-20260927/Kinakaze/rootfs --dist baseline=artifacts/proc-agent-dist --dist candidate=artifacts/goal-perf-candidate --output artifacts/goal-daily-comparison --repeat 3 --warmup 1
```

每轮打乱场景顺序、交替候选版与基线先后顺序。两个 init 各自持有状态。工作流耗时包括 Bash、子进程和清理；不是单个 syscall 或单个命令的纯执行耗时。当前二进制为 dev 构建，不能把收益直接外推到 release、Clang 或完整 agent 工作流。

`artifacts/goal-daily-comparison/results.json` 的 56 个样本全部通过（每版每场景一次预热、三次正式采样）。正式样本中位数：

| 工作流 | 基线 ms | 候选 ms | 耗时降低 |
|---|---:|---:|---:|
| 20 次进程启动 | 6896 | 3239 | 53.0% |
| 文本处理 | 2500 | 1224 | 51.0% |
| 文件树复制与检查 | 3023 | 1665 | 44.9% |
| 压缩解压 | 3640 | 1942 | 46.6% |
| 进程查询 | 2075 | 1045 | 49.6% |
| 包查询 | 1392 | 704 | 49.4% |
| Python 文件处理 | 1240 | 732 | 41.0% |

独立诊断运行前后各 47 条 `providers-discover` 记录，中位数从 95.474 ms 降到 26.280 ms；日志目录分别为 `goal-daily-startup-profile` 和 `goal-daily-startup-profile-after`。包括 fork/exec 涉及的多个 worker，不能将 47 当作 47 次独立工作流。正式性能对照不启用诊断写日志。

## 已验证与变更

- `artifacts/goal-agent-performance-ownership-baseline/result.json`：init 资源托管回归通过，覆盖创建者退出、跨 worker 访问、挂载和资源清理。没有改为 worker 持有全局可变状态。
- `artifacts/goal-daily-baseline-cleanup/results.json`：七类场景各一次预热、三次正式测量全部通过。
- 分阶段计时 `artifacts/goal-daily-startup-profile/candidate/profile` 显示优化前单个 worker 的 `providers-discover` 可达约 97 ms，而对应 `/bin/true` AOT 缓存应用约 0.8 ms。
- `crates/bridge/src/native.rs` 将 PE 导出名扫描换成标准库的有界字节搜索，保留 4096 字节名字上限、映像区段检查和 UTF-8 验证。仍校验所有 Rust 私有导出，不新增共享缓存或 worker 全局状态。17 项 bridge 测试通过，包括新增长度上限与跨区段拒绝测试。
- 原生 `rmdir` 统一使用 POSIX disposition，持有目录句柄时立即删除名称并保留 inode；新回归验证同名重建、旧 inode 存活及非空目录拒绝。单元测试通过。该改动不等于修复下面的 GNU rm 场景。
- 五项现有代码入口分析测试通过；新增 absolute jump-table 扫描的完整边界覆盖仍待补充。
- 候选版 `artifacts/goal-perf-ownership/result.json` 资源托管回归通过，包括强制终止原生 worker 后挂载数据仍可读取。
- 候选版 `artifacts/goal-perf-compatibility/report.json` 离线回归通过：proc、文件系统边界、Bash 中断、编辑样例与四项测试。离线脚本的 `agent_edit_passed` 只表示确定性编辑夹具通过，不是在线模型或 Claude/Codex/pi 的新验收。

## 未解决的复现与范围

`mkdir "$p"; cd "$p"; rm -rf "$p"` 在旧版和候选版都返回 EBUSY。Python 直接 `os.rmdir` 的目录句柄/cwd 测试在两版均通过（分别验证 `/tmp` 和 `/root`），故不能把失败简单归因于 rmdir 或 Windows 当前目录锁。复现记录见 `artifacts/goal-directory-removal-native/results.json`。性能脚本先 `cd /` 再清理；原始失败保留在 `artifacts/goal-daily-baseline`。

systemd 空闲 CPU、Clang 中型项目、Codex/pi 真实编辑工作流、剩余 syscall 语义和跨 worker 句柄边界仍需继续验证。此前 Claude 真实编辑与 Ctrl+C 结果对应旧构建，不能自动算作新候选版已验收。

## 扩展日常命令与 release 对照

`tools/benchmark-daily-commands.py` 已扩展至 15 类，新增管道、4 MiB 哈希、硬链接/软链接、Git 提交/恢复、Node 文件与计时器、权限/截断/时间戳、并行 xargs、cut/uniq/tr/paste。包含 Git 和 Node 的完整集合需要已安装相应包的开发根。

新代码优化：`epoll.rs` 对不涉及注册项的 close/rebind/readiness-consumed 操作，以及没有改变 reported 位的空闲扫描，避免触发 `SetsGuard::DerefMut`。此前即使内容没变，也会编码所有共享 epoll 集合再比较；现在保持只读。共享锁、跨 worker 刷新、真实 edge 变化的发布和唤醒仍保留。没有引入 worker 权威状态或更改轮询周期。

产物：dev `artifacts/goal-epoll-publication-candidate`；release `artifacts/goal-daily-optimized-release`（同一代码，启用现有 opt-level 3、包含开发 ELF 导入库）。release 使用 `tools/build.ps1 -Release -Development -NativeOnly` 构建，供现有 Debian 根使用，不是完整新装 Debian 归档。

```powershell
python tools/benchmark-daily-commands.py --root artifacts/goal-systemd-idle/debian-root --dist baseline=artifacts/goal-wait-views-candidate --dist candidate=artifacts/goal-epoll-publication-candidate --dist release=artifacts/goal-daily-optimized-release --output artifacts/goal-daily-expanded-comparison --repeat 3 --warmup 1 --timeout 900
```

共 180 个样本，每版每场景一次预热、三次正式采样。新候选 dev 和 release 各 60/60 通过；旧 baseline 为 59/60，第一次正式 archive 样本发生 `tar: gzip: Cannot waitpid: No child processes`，日志另有 parent-death 查询失败。不能认定新优化修复了这一偶发 waitpid 问题，也不能把整次比较报告说成全部通过。

相同代码 dev → release 的正式样本中位数如下。**主要收益是编译配置差异，不是本轮 epoll 优化的独立收益，也不是相对旧 release 的加速。**

| 工作流 | dev ms | release ms | 耗时降低 |
|---|---:|---:|---:|
| 20 次启动 | 3416.0 | 1485.7 | 56.5% |
| 文本处理 | 1281.9 | 575.8 | 55.1% |
| 文件树 | 1725.3 | 832.1 | 51.8% |
| 压缩解压 | 2068.4 | 940.6 | 54.5% |
| 进程查询 | 1140.0 | 511.9 | 55.1% |
| 包查询 | 767.5 | 348.2 | 54.6% |
| Python | 776.3 | 374.9 | 51.7% |
| 管道 | 1318.9 | 616.2 | 53.3% |
| 哈希 | 1182.6 | 577.0 | 51.2% |
| 链接 | 1397.7 | 604.8 | 56.7% |
| Git | 3124.1 | 1530.9 | 51.0% |
| Node | 1806.3 | 500.0 | 72.3% |
| 元数据 | 1925.1 | 884.6 | 54.0% |
| 并行 xargs | 2423.7 | 1035.1 | 57.3% |
| 文本列处理 | 1340.0 | 610.3 | 54.5% |

同为 dev 的 baseline/candidate 日常工作流没有显著一致的差异。`artifacts/goal-systemd-publication-comparison` 的四轮空闲采样中，候选约 14.7%–16.9% 单核，基线约 13.1%–20.3%；存在随时间变化的噪声，不据此声称可靠的百分比优化。

同代码 dev/release 的 systemd 对照见 `artifacts/goal-systemd-release-comparison`，顺序 dev/release/release/dev，每轮 15 秒、三段采样，四轮最小目标启动和关机均通过。dev 样本约 14.4%–19.4% 单核，release 约 4.4%–8.4%。仍非零空闲开销，timerfd/signalfd/inotify 的完整事件等待尚未完成。

新增 `tools/benchmark-clang-builds.py` 顺序驱动现有 33 翻译单元探针，保留各版目录、编译日志、二进制哈希和构建次数/输出校验，不并发运行不同版本以免污染计时。

`artifacts/goal-clang-release-comparison/results.json` 两版全部阶段通过：

| Clang 14、32 源文件 + main、4 并发 | dev 秒 | release 秒 | 编译单元数 |
|---|---:|---:|---:|
| 干净构建 1 | 52.586 | 13.318 | 33 |
| 干净构建 2 | 53.246 | 13.428 | 33 |
| 无改动 | 0.274 | 0.189 | 0 |
| 单源文件增量 | 4.278 | 1.554 | 1 |
| 共享头修改 | 52.588 | 13.252 | 32 |

使用相同 Clang、源码生成器、并发数和现有 ELF SDK。每个阶段验证构建输出和最终程序数值；干净构建是删除目标文件，不清空 OS 缓存。这是一个固定项目场景，不表示所有 C++ 项目都有相同加速。

最终验证：43 项相关 dev 单测通过（epoll 28、eventfd 6、timerfd 3、inotify 6）。release 的 `goal-release-shared-waits` 验证跨 worker timerfd/inotify、Unix 对端退出和挂载通知；`goal-release-kernel-ownership` 验证创建者退出/强杀后的 init 状态保活、命名空间与清理；`goal-optimized-release-compatibility` 验证 proc、文件系统边界、Bash 中断和离线编辑；`goal-pi-optimized-release` 验证真实 pi CLI + 本地确定性服务的 read/bash/edit。上述均通过，仍未代替在线 Claude/Codex/pi 全场景测试。

针对 archive 的额外十轮 baseline/release 交替复测，`artifacts/goal-archive-wait-stress/results.json` 的 20 个样本全部通过，未再次复现 ECHILD。原始失败记录仍保留，不能以复测通过认定竞态已经修复。
