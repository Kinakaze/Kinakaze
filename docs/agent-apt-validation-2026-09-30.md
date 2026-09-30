# Agent 场景与本地 APT 安装验证（2026-09-30）

本轮重点检查 zcode、dsh、pi、Claude Code、Codex、AstrBot、Bun 和 Node.js，并将 APT 下载与本地安装分开计时。原始证据位于 `artifacts/agent-scenarios-20260930/`；下文证据路径均相对此目录。

最终测试产物为 `candidate-r2`，发布构建完成 29 个原生模块及 5914 个 guest exports 检查；`cargo fmt --all -- --check`、`git diff --check` 通过。

## 应用结果

`final-suite.json` 中的 11 个串行任务全部返回成功。五个 Agent 两轮共 164 项真实工具操作通过。

| 应用 | 覆盖与结果 | 证据 |
|---|---|---|
| Codex | 21 项 × 2，含 MCP | `final-control/results.json` |
| Claude Code | 21 项 × 2，含 MCP | `final-control/results.json` |
| pi | 16 项 × 2 | `final-control/results.json` |
| zcode | 12 项 × 2；基线两次崩溃，修复后两次完成 | `final-zcode-0`、`final-zcode-1` |
| dsh | 12 项 × 2 | `final-dsh-0`、`final-dsh-1` |
| Node.js 22 / 24 | 各两轮；VM、压缩、加密、worker/Atomics、文件、子进程、TCP 和 Unix socket | `final-node22`、`final-node24` |
| Bun | 文件操作两轮；每轮 4 路并发共 32 个子进程，检查管道、退出状态、Unicode cwd | `final-bun` |
| AstrBot | 两次服务启动、登录鉴权、共 32 次 API 请求、SQLite 状态及重启 | `final-astrbot` |

Agent 工具流程包括原始文件读取、失败测试、拒绝错误编辑、修改源文件和 Unicode 文件、重新测试、stderr 与大输出；具体项目以各 CLI 报告为准。

`final-resources/results.json` 的三个检查点均为 269 个句柄、5 个线程、156430336 字节私有提交。`final-memory/report.json` 的内存专项通过。zcode、dsh 和内存专项均报告清理通过、无资源阈值触发、退出后无所属进程存活。Windows Job 的原始峰值可能包含被拒绝的投机分配，不能当作实际驻留或实时私有提交。

## 启动与 Node 复测

`benchmark-startup.py` 交替运行基线和候选，每种先各预热一次、再各测三次，使用独立临时 HOME 与清空后的 guest 环境执行 `--version`。计时包含 worker/init、加载和退出，是 CLI 命令完成时间，不是 GUI 就绪或清空宿主缓存后的冷启动。全部 48 个正式启动样本与 16 个预热样本通过。

| CLI | 基线中位数（秒） | 候选中位数（秒） | 证据目录 |
|---|---:|---:|---|
| Codex | 0.919 | 0.925 | `startup-codex` |
| Claude Code | 0.836 | 0.830 | `startup-claude` |
| pi | 4.438 | 4.135 | `startup-pi` |
| zcode | 1.198 | 1.221 | `startup-zcode` |
| dsh | 0.674 | 0.674 | `startup-dsh` |
| Node.js 22 | 0.413 | 0.407 | `startup-node22` |
| Node.js 24 | 0.410 | 0.397 | `startup-node24` |
| Bun | 0.335 | 0.332 | `startup-bun` |

本轮主要收益是本地安装与崩溃修复；启动结果整体接近基线，pi 的这组样本有所缩短。

整套回归中 Node 22/24 的完整探针曾分别出现 2.273/3.398 秒中位数，高于较早基线的 1.039/1.391 秒。随后用上述交替方法直接运行同一 `NodeRuntimeProbe.js`，Node 22 的基线/候选为 0.970/0.976 秒，Node 24 为 1.050/1.090 秒，全部通过，未复现成倍增长；原始慢样本保留。证据为 `startup-node22-runtime`、`startup-node24-runtime`，每个产物各三次正式样本及一次预热。

最后用原来的 `test-common-workloads.py` 完整入口再交替执行三轮，12 次运行全部通过：Node 22 基线/候选中位数为 1.217/1.216 秒，Node 24 为 1.284/1.262 秒。该复核保留 shell、fixture 和退出清理开销，证据为 `node-recheck.json` 及 `recheck-node*`。

## 修复与行为

### zcode 的 XSAVE 状态恢复崩溃

基线的两次 zcode 工具流程均在执行中崩溃。诊断发现异常来自生成的 raw syscall trampoline 中的 `XRSTOR`。保存区位于也供普通 host call 使用的栈上，`XSAVE` 不会清空标准格式头部的全部保留字节；旧栈数据留在 `XCOMP_BV` 或保留字段中会使恢复触发异常。

`emit_save_extended_state` 现在先用通用寄存器清零 64 字节头部，再保存扩展状态，避免修改尚未保存的向量寄存器。原来的嵌套调用测试增加了对外层和内层保存区的非零污染，同时继续验证 SIMD 数据、参数、返回值和栈恢复。

- 修复前：`xsave-before.json` / `xsave-before-r3.log`，污染栈回归失败。
- 修复后：`xsave-after.json`，同一回归通过。
- 首次真实工作流复核：`r1-zcode/report.json`，12/12 工具操作通过。

状态格式依据：[Intel 指令集手册，XSAVE/XRSTOR](https://cdrdv2-public.intel.com/789589/334569-sdm-vol-2d.pdf)。

### APT/dpkg 的文件写回

原 `sync_file_range` 忽略范围和 flags，总是执行与 `fsync` 相同的完整刷盘。现在验证参数和文件类型；flags 为零不执行 I/O；普通文件使用 `NtFlushBuffersFileEx(FILE_DATA_ONLY)` 提交数据，避免每次范围写回都额外同步设备缓存。Windows 实现仍可能同步等待整文件写回。

文件描述符会先固定到当前打开的 inode，重命名或描述符复用不会把请求转移到同名新文件。每次异步原生请求使用独立重开句柄，返回前完成或回收请求。挂载层保留原有 overlay/volatile 错误处理；不支持数据写回的文件系统回退到完整 flush。打开后改为只读导致重开写句柄失败时，沿用原描述符的完整 flush 行为。

`fsync`、`fdatasync` 和 dpkg 的持久化调用保留。未设置 `force-unsafe-io`、未引入忽略同步的预加载库，也未修改系统 APT/dpkg 配置。范围写回本来就不等于持久化屏障，参见 [Linux 接口说明](https://man7.org/linux/man-pages/man2/sync_file_range.2.html) 与 [Microsoft 数据写回接口](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntflushbuffersfileex)。

## 底层回归

`native-focused-final.json` 的 8 组共 104 项通过、2 项性能测试忽略；嵌套子进程重复执行的同一测试不重复计数。覆盖 ptrace、映射生命周期、syscall/futex、SysV IPC、trap/trampoline、文件元数据和写回。

`overlay-sync.json` 另有 fsync 描述符测试 1 项、overlay 挂载测试 39 项通过，2 项基准忽略。包括严格 copy-up 同步、volatile 存储错误持续返回、只读挂载与已打开 inode 的身份保持。

写回专项覆盖重命名后原 inode、同名替换文件、文件偏移保持、8 种 flags 的重复执行、负范围、溢出、无效标志、无效 fd、管道和打开后设为只读。

## APT 计时方法

新增 `tools/benchmark-apt-install.py` 建立临时 loopback HTTP 仓库，生成两个有依赖关系的 `.deb`：600 个小文件和一个 8 MiB 不可压缩数据文件。每轮使用独立的 APT 状态、缓存、日志和 dpkg 数据库/安装目录，不操作客体系统包数据库。

每轮顺序执行 update、仅下载、`--no-download` 安装、`--no-download --reinstall`、purge、audit。安装和重装后检查全部内容哈希、两个包的 installed 状态、维护脚本输出；重装前删除脚本标记，确认脚本确实再次执行；卸载后检查 payload 目录消失。

基线与候选交替顺序，各执行三轮。安装、重装数据包含真实 APT/dpkg 与维护脚本开销，排除下载时间。宿主缓存未强制清空，数字是本机重复测量，不是冷磁盘或所有软件包的保证。候选产物还包含工作区同期的 inode 元数据与 syscall 变更，性能数字用于比较整体产物，不能全部归因于范围写回这一项。

三轮对比均通过，证据为 `apt-comparison/results.json`。每个产物完成三次安装、三次重装、三次卸载和三次 audit。单位为秒：

| 阶段 | 基线中位数 | 候选中位数 | 耗时减少 | 基线范围 | 候选范围 |
|---|---:|---:|---:|---:|---:|
| 本地安装 | 12.021 | 9.055 | 24.7% | 11.700–15.258 | 8.865–9.577 |
| 本地重装 | 14.230 | 10.348 | 27.3% | 12.550–18.076 | 10.142–14.664 |
| 卸载 | 3.950 | 3.658 | 7.4% | 3.844–3.987 | 3.649–4.395 |

较早的 `apt-diagnostic` 在旧实现中统计了同步调用，整组安装/重装/卸载累计 `fsync` 1365 次约 1.15 秒、`sync_file_range` 2408 次约 1.93 秒。插桩运行用于定位，不参与上表。这也说明本地耗时包含大量 inode 操作及子进程开销，不能把所有耗时归结为同步刷盘。

## 重跑入口

```powershell
python tools/benchmark-apt-install.py --root artifacts/goal-systemd-idle/debian-root --dist baseline=artifacts/agent-scenarios-20260930/baseline --dist candidate=artifacts/agent-scenarios-20260930/candidate-r2 --repeat 3 --output artifacts/apt-comparison-new
python tools/test-syscall-ptrace.py --target-dir target/agent-scenarios-tests --suite focused --output artifacts/native-tests-new.json
```

应用流程使用仓库中的 `test-agent-compatibility-matrix.py`、`test-openai-agent-workflow.py`、`test-common-workloads.py`、`test-astrbot.py`、`test-agent-resource-lifetimes.py` 和 `test-agent-memory-advice.py`。各报告保存实际命令、退出状态、日志及运行库哈希。

模型协议测试使用本地确定性服务与假凭据，验证真实 CLI 工具行为。它不覆盖真实账号登录、在线模型质量或所有第三方插件；有界重复测试也不能证明任意时长运行不会崩溃或泄漏。
