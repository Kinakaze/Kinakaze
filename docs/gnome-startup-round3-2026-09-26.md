# GNOME 第三轮启动压缩（2026-09-26）

本轮消除了 Shell 初始化中的两个实际等待：同步激活 AccountsService，以及已经连接 IBus 后仍重复启动 ibus-daemon。相同环境下，成功样本的启动中位数 **4.029 → 3.427 秒，减少约 15%**。新版 7/7 启动成功；旧版基线 6/7 成功，另一次在 IBus 阶段退出，完整保留失败记录。

这是本轮交替测量的结果，不能与上一轮的 6.51 秒直接相减。文件缓存已热但未受控清空，宿主负载有变化；计时终点是 Shell `startup-complete` 日志，不是屏幕首帧或键鼠响应。

## 瓶颈证据

使用从已安装 GNOME Shell 43.9 ELF 提取的原始 GResource JS，仅在诊断运行中加入客体 `GLib.get_monotonic_time()` 标记。

1. `SystemActions` 构造 `AccountsService.UserManager` 的初始化段阻塞 **0.747 秒**，对应 system bus 激活 AccountsService 及其 PolicyKit 依赖。把激活请求提前到 IBus 初始化前，同段降至 **0.025 秒**，另一次为 0.011 秒。提前激活仍做相同工作，不能把这段差值全部当成端到端收益。
2. `IBusManager._queueSpawn()` 等待 systemd 查询后，无条件启动非 systemd 管理的 ibus-daemon。诊断证明进入 `_spawn()` 时 **`this._ibus.is_connected()` 已是 true**，重复创建进程仍阻塞 **0.472 秒**，随后新 daemon 输出 `current session already has an ibus-daemon.` 并退出。它发生在首次主循环里，推迟了低优先级的启动动画准备回调。

第二轮诊断包装器的全局字符串替换曾把 `G_RESOURCE_OVERLAYS` 拼成错误变量名，导致那次 `--overlay` 未生效。第三轮改为独立变量赋值，并核对 GLib 的 overlay 加载日志和客体时间标记；上一轮不使用 overlay 的正式基准不受影响。首次复制诊断模块遇到 Windows 默认编码错误，修正为 UTF-8 后重新运行；只对完整诊断的日志作细分分析。

## 改动及回退

- `tools/run-gnome.py`：在本次私有 system bus 上异步发送 AccountsService 激活请求，与 IBus 初始化重叠。未增加等待服务就绪的步骤；服务不存在或激活失败时，Shell 仍走原有路径。
- `tools/gnome_ibus.py`：只对识别到的 `_queueSpawn()` 方法增加连接状态检查。检查放在异步 systemd 查询之后，因此等待期间建立的连接也能避免重复启动。未连接时的 daemon 启动、Wayland 参数和主动 `restartDaemon()` 均保留。
- `tools/prepare-desktop.py`：提取已安装的原始资源，生成上述单行改动的覆盖文件及 manifest，不改写上游 ELF。覆盖只映射 `ibusManager.js` 这个资源。未知模块布局或缺少提取工具时保持原始代码。
- 启动器校验 Shell ELF 和生成资源的 SHA-256；Shell 更新、资源修改、manifest 缺失或损坏时忽略覆盖。显式设置的 `G_RESOURCE_OVERLAYS` 优先。

准备已有 root 时，重新执行常规 `prepare-desktop.py` 即可生成覆盖；此前准备好的 root 即使没有该文件，也能照常启动并获得 AccountsService 调度优化。本轮仅准备隔离测试 root，未修改 `promo-v1/gnome-root`。

## 正式对照

固定运行包 `artifacts/startup-extreme4-20260926/final-dist`，没有重编译原生库；共用 `artifacts/gnome-speed-20260926/root` 的相同时区、locale、缓存和偏好。该测试根此前已关闭动画，两版相同，本轮代码不修改此偏好。

每次在独立的非输入 Windows 桌面运行，不调用 `SwitchDesktop`。观察上限 24 秒，外层独立 Windows Job watchdog 为 27 秒；就绪后保留 1 秒。正式比较关闭诊断 JS/native profiling；优化版的单资源功能覆盖启用。所有运行均在 30 秒内退出，清理后的 Job 活动进程数为 0。

| 配对 | 基线（秒） | 新版（秒） |
| --- | ---: | ---: |
| 1 | 3.862 | 3.325 |
| 2 | 5.834 | 3.427 |
| 3 | IBus 退出 255，未启动 Shell | 3.489 |
| 4 | 4.054 | 3.670 |
| 5 | 3.756 | 3.368 |
| 6 | 4.016 | 3.368 |
| 7（补充对照） | 4.042 | 3.692 |
| 成功样本中位数 | **4.029（6 个）** | **3.427（7 个）** |

第 3 组失败没有作为 0 秒计入速度统计。六组双方均成功的配对，中位数为 4.029 → 3.397 秒；逐对差值中位数为 0.463 秒。所有尝试都保留在 `comparison.json` 中。基线的 IBus 提前退出在前面的诊断中也出现过一次；尚未确定根因，本轮没有加自动重试，也不声称已修复这个偶发问题。

Mutter 运行到 Shell 就绪的成功样本中位间隔由 **2.275 → 1.713 秒**。剩余耗时包括解释器/总线/IBus 启动、Shell 原生装载、JS 导入、界面与背景准备及宿主调度，不能全部归为磁盘等待。这是继续压缩时应关注的阶段。

## 回归

- **11 项 Python 检查**：包含时区、计时及新增的 ELF/覆盖内容失效校验、缺失/损坏 manifest 回退、重复准备和重启代码保留。
- **7 项实际生成 JS 模块的逻辑检查**：已连接、等待期间连接、未连接、systemd 管理、主动重启、systemd 重启路径和 Wayland 启动参数。使用受控 IBus/systemd 状态，不启动 GUI。
- `merged-integration`：真实 Shell、`--app`、session/system buses 和自动激活服务环境通过；连接实际 IBus 私有总线，确认 Shell 持有 panel 名称并成功创建输入上下文。Shell 3.550 秒就绪，6.753 秒清理完毕。
- `fallback-service-missing`：故意把提前激活请求指向不存在的服务，Shell 仍启动，应用和 IBus panel/输入上下文检查通过；9.103 秒清理完毕。
- `failure-ibus`：注入 IBus 退出 1，未启动 Shell，0.904 秒清理完毕，包括可能已经提前启动的服务。
- `limit-cleanup`：探针阻塞 60 秒，24 秒观察上限生效，24.148 秒清理完毕。

以上实际会话回归清理后的 Job 活动进程数均为 0。没有验证物理键鼠输入、屏幕首帧或完整 GNOME session manager。

## 工件与复现

`artifacts/gnome-speed-round3-20260926/` 保存冻结的基线/新版启动器、提取的上游资源、所有诊断和失败日志、正式运行报告、源码 SHA-256、manifest 与比较结果。计时受宿主负载影响，最快值不是启动时间承诺。

```powershell
python tools/test-gnome-startup.py -v
node tools/test-gnome-ibus.js artifacts/gnome-speed-20260926/root/usr/share/kinakaze/gnome-shell/ibusManager.js
# 在上述已有测试根和分发上复现；tag 必须是新目录名。
python artifacts/gnome-speed-round3-20260926/run.py --launcher tools/run-gnome.py --tag repeat-check --app-probe --ibus-probe --hold 3
```
