# GNOME 启动诊断与修复（2026-09-26）

第一轮确认了缺少时区配置导致 GNOME 日历首次本地日期转换阻塞约 2 秒的问题，补齐图标主题缓存准备和正式启动计时。第二轮进一步缩短进程启动链路，并完成隔离桌面的交替测量：Shell 启动前中位耗时 2.18 → 1.75 秒，完整启动中位耗时 6.72 → 6.51 秒。**宿主负载波动明显，完整启动的收益较小，不能宣称固定加速比例。**

后续的 AccountsService 等待及重复 IBus 进程优化另见 [第三轮记录](gnome-startup-round3-2026-09-26.md)，该批成功样本中位数 4.03 → 3.43 秒。各轮基线负载不同，不作跨批次直接比较。

## 测试环境和测量口径

- Release 运行包：`artifacts/startup-extreme4-20260926/final-dist`，本轮未替换其二进制。
- 从 `artifacts/promo-v1/gnome-root` 复制独立测试根到 `artifacts/gnome-speed-20260926/root`。只修改该副本中的时区、缓存及实验设置，未修改原演示根。
- 原始启动器保存为 `artifacts/gnome-speed-20260926/baseline-launcher.py`。
- `measure.py` 在宿主读取完整日志行，记录从请求启动到 Shell `startup-complete` 日志的时间。实验采样间隔 10ms；正式启动器采样间隔 20ms，均另有调度延迟。
- 细分诊断通过从已安装 ELF 提取的 GResource JavaScript 添加计时标记，仅放在测试根的 `/tmp/gnome-speed-ui`。未修改上游 GNOME 包，也未将诊断覆盖写入正式启动器。
- 文件缓存未受控清空，宿主负载有变化，细分 profiling 有额外开销，不能把不同批次直接视为配对基准。

## 已获得的证据

| 运行 | 观察 |
| --- | --- |
| `baseline-1` | 新复制根的首次诊断运行，开 loader profiling，45 秒内未见 Shell 就绪；不能解释成机器冷启动耗时 |
| `baseline-2` | 不开 profiling，Shell 就绪 7.353 秒；其中 Shell 启动前 1.412 秒 |
| `baseline-js` | 带 JS 诊断：顶栏构造约 2.63 秒，启动动画约 1.10 秒 |
| `panel-js` | 日历菜单构造约 1.75 秒，占该轮顶栏大部分时间 |
| `calendar-js` | 阻塞定位到日历页眉首次本地日期转换，约 1.96 秒 |
| `timezone-fixed` | 配置 `Asia/Shanghai` 后，上述日期转换低于宿主 10ms 采样分辨率；该轮 Shell 就绪 5.795 秒 |

缺时区的测试根既无 `/etc/timezone`，也无 `/etc/localtime`。为同一根写入已安装的 `Asia/Shanghai` zoneinfo 并设置 `TZ` 后，日期转换停顿消失。原日志也从数字偏移时区变为 `China Standard Time`。这是配置瓶颈的定位证据；5.795 与 7.353 秒使用不同诊断设置和时间批次，不能据此声称一个固定的端到端提升百分比。

`Adwaita`、`hicolor` 最初都没有 `icon-theme.cache`，客体 `gtk-update-icon-cache` 已分别成功生成。图标缓存单独效果尚未完成交替测量，不能将顶栏的主要停顿归因于图标扫描。

测试根另缺少 `/usr/lib/locale/C.utf8/LC_CTYPE`，存在 locale 警告。这是原根的配置缺口；准备新根应包含锁定的 `libc-bin` 数据，不能用修改 locale 返回值来假装支持。

## 正式改动

- `tools/prepare-desktop.py`：根据已配置的时区补齐缺少的 localtime；完全未配置的根默认 `Etc/UTC`，已有 localtime 保持不变。增加 GTK 图标主题缓存生成。
- `tools/run-gnome.py`：增加 Debian dist-packages 搜索路径；增加会话专用 `--timezone` 和无时区时的显式 UTC 默认值；记录 guest、system bus、session、IBus、Mutter、Shell 各阶段。`startup_seconds` 来自 Shell 启动完成日志，`seconds` 仍表示总观察时长。
- `--stop-after-ready` 为可选的一次会话观察上限，必须同时给出正数 `--timeout`；默认交互启动行为保持。所有退出路径仍由原有 Windows Job 清理自己的进程树。
- `tools/test-gnome-startup.py`：8 项无图形检查通过，包括时区保留/补齐/路径校验、分段日志、重复标记、退出与超时区分、嵌入客体脚本语法。Python 编译检查及补丁空白检查通过。

## 未合入的实验及宿主干扰

第一轮减少 Python 包装层的实验中，IBus 提前退出；并行启动 Shell/IBus 的耗时也不稳定，最后一次会话提前退出。因此这些版本仅留在 artifacts，没有合入正式脚本。第二轮使用不同的进程层级，仍等待 IBus 就绪后再启动 Shell，见下文。实验中关闭动画仅改了独立测试根的偏好，正式代码没有关闭动画或删减桌面服务。

连续启动全屏桌面期间，用户报告屏幕闪烁、卡住。立即停止图形测试，检查时已经没有测量启动器或 worker 残留，用户随后确认屏幕恢复。Windows 应用事件在同一时段记录了 `nvcontainer.exe` / `NvBackend64.dll` 和 `ASUSSmartDisplayControl.exe` 崩溃；这只能证明同期存在显示相关组件异常，尚不能确定驱动崩溃与测试之间的具体因果。

该轮随即停止图形会话。没有修复或更改宿主驱动；实验截图选中了小辅助窗口，不能作为桌面已正确绘制的证据。用户随后再次授权启动、要求 30 秒内结束；单次前台试运行在 12.386 秒收到 Shell 就绪日志，25.215 秒结束进程树。该试运行的条件不同于后面的隔离桌面基准，不能直接计算二者的优化比例。

原始逐行时间线、stdout/stderr、分阶段数据及未采用的实验全部保存在 `artifacts/gnome-speed-20260926/`。

## 第二轮：缩短会话启动链路

原先的链路是 Python → dbus-run-session → 第二份 Python → Shell。新版由 dbus-run-session 启动一份 Python；在 system bus 启动期间导入 GI，准备好环境、IBus 和应用后，直接 exec 为 Shell，省去重复解释器初始化和 Shell 的 fork。dbus-run-session 继续等待该客体 PID，宿主 Windows Job 负责收回所有服务、应用和重新挂接的子进程。正常 exec 不执行 Python finally，因此会话环境文件清理也移到宿主的 finally 中。

session bus 比 system bus 更早启动，必须在使用 GSettings 之前调用 `UpdateActivationEnvironment`，发布最终的 system bus 地址、IBus 地址、运行目录、时区和桌面环境。回归实际激活了一个 D-Bus 服务，由该服务返回其环境并连接 system bus，再与 `--app` 应用的环境比较，验证这条路径没有串会话。

`close_fds=False` 的实验没有证明可靠收益，没有合入。仅合并 Python 包装层的中间版本也没有单独证明完整启动加快；最终测量的是包含 Shell exec 的完整方案。第一轮的并行 Shell/IBus 方案仍未采用。

### 测量条件与结果

第二轮文件位于 `artifacts/gnome-speed-round2-20260926/`。`baseline-launcher.py` 保存本轮开始时的正式启动器，`session-exec.py` 保存最终候选，`comparison.json` 记录逐次结果、顺序和源码哈希。合入的客体脚本与最终候选的 Python AST 一致。

- 仍使用原 release 分发 `startup-extreme4-20260926/final-dist`，没有重编译或替换原生库。
- 两版共用相同测试根、`Asia/Shanghai`、字体/图标缓存及已关闭动画的实验偏好。正式启动器没有修改动画偏好。
- 测试前从已校验的锁定 `libc-bin` 包补齐该副本缺少的 12 份 `C.utf8` 文件；新旧版本共用这些数据，不把 locale 的变化算作本轮启动器收益。来源见 `locale-inputs.json`。
- `CreateDesktopW` 创建独立的非输入 Windows 桌面，未调用 `SwitchDesktop`；不测量主桌面的首帧或交互。所有会话观察上限 24 秒，外层独立 Job watchdog 为 27 秒。
- 正式比较未开启 native profiling 或 JS 诊断覆盖；收到 Shell 就绪日志后保留 1 秒，再结束自己的进程树。文件缓存未清空，宿主负载仍有变化。

按 A/B、B/A 交替次序运行六组，每个单元格为 Shell 就绪秒数；所有十二次均成功启动并存活到观察结束。

| 配对 | 基线 | 优化版 |
| --- | ---: | ---: |
| a5 / d1 | 11.302 | 9.828 |
| a6 / d2 | 9.944 | 9.096 |
| a7 / d3 | 6.048 | 4.514 |
| a8 / d4 | 6.615 | 6.568 |
| a9 / d5 | 6.184 | 6.454 |
| a10 / d6 | 6.817 | 6.387 |
| 中位数 | **6.716** | **6.511** |

Shell 启动前中位耗时从 **2.177 → 1.745 秒**，减少约 0.43 秒（该批次约 20%）；完整启动中位数仅减少约 0.21 秒（约 3%），平均值 7.818 → 7.141 秒。保留了所有六组数据，没有排除较慢结果。负载波动远大于完整启动的中位差，不能将其作为普遍的固定提速承诺，也不能把最快的 4.514 秒当成典型成绩。

优化后从 Mutter 运行日志到 Shell 就绪的中位间隔仍为约 4.15 秒，剩余耗时主要落在 Shell 加载和桌面初始化阶段；这一间隔也包含服务活动和宿主调度，不能全部归为某一项 CPU 或磁盘开销。先前的原生 profiling 有额外开销，仅用于定位，未纳入上述基准。

### 合入后的功能与清理检查

- `final-app-integration`：真实 Shell 与 `--app` 探针一起运行；IBus、两个 D-Bus、激活服务环境全部通过。Shell 6.386 秒就绪，观察 3 秒后于 9.671 秒结束，外层 Job 活动进程数为 0。
- `final-exit-status`：exec 后的客体返回 7，dbus-run-session 保留退出码，宿主报告错误；3.267 秒结束，Job 活动进程数为 0。
- `final-ibus-exit`：注入 IBus 返回 1，启动器报告失败且未启动 Shell；1.238 秒结束，Job 活动进程数为 0。首次故障注入使用了根中不存在的可执行文件，其错误退出记录另保留在 `final-ibus-failure`。
- `final-timeout`：探针完成总线检查后阻塞 60 秒，24 秒观察上限生效，24.206 秒清理完毕，Job 活动进程数为 0。
- 8 项原有无图形测试、嵌入脚本/Python 编译检查通过。此轮没有进行键鼠输入、首帧、完整 GNOME session manager 或所有桌面服务验收。

复现一轮限时隔离桌面检查（仓库根目录执行，使用上述既有测试根和分发；tag 必须是新目录名）：

```powershell
python artifacts/gnome-speed-round2-20260926/run.py --launcher tools/run-gnome.py --tag repeat-check --app-probe --hold 3
```
