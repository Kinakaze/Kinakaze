# GNOME Shell 开发会话

当前可运行 GNOME Shell 43.9 / Mutter 43.8 的 X11 桌面，使用 Kinakaze 的原生图形后端及隔离的 D-Bus system/session buses。分发目录与 Debian 客体根目录分开，避免混入不同版本的 ELF 依赖。

## 启动

在仓库根目录执行，已准备好的开发环境使用：

```powershell
python tools/run-gnome.py --dist artifacts/gnome-full-dist --root artifacts/gnome-startup-root --report artifacts/gnome-session.json
```

默认保持会话运行；Ctrl+C 结束该次会话创建的进程树。自动检查可加 `--timeout 90`，启动器会观察 90 秒后结束自己的会话并记录结果。报告中的 `process_alive_at_timeout` 表示观察结束时进程仍在运行，不代表所有桌面功能通过。

依赖来自 `tools/guest-deps/dependencies.lock.json` 的 `gnome-shell` 包闭包。新根目录通过 `tools/prepare-root.py --package gnome-shell` 安装锁定的包；使用同一 Debian 版本的源目录。随后运行客体工具生成 schemas、图标/图片、GIO、MIME、字体和 IBus 缓存：

```powershell
python tools/prepare-desktop.py --dist artifacts/gnome-full-dist --root artifacts/gnome-startup-root --timezone Asia/Shanghai
```

构建原生分发前先关闭使用该分发的会话，避免覆盖正在加载的 DLL：

```powershell
./tools/build.ps1 -DistDirectory artifacts/gnome-full-dist -RefreshExports
```

## 图形与输入验收

Composite overlay 使用无边框、屏幕大小的窗口，客户区原点与 X root 原点重合；桌面不占用 Windows 的全局 TOPMOST 层级。普通应用窗口和弹出窗口保留各自窗口样式。

```powershell
python tests/guest/tool-matrix.py --worker artifacts/gnome-full-dist/worker.exe --root artifacts/gnome-startup-root --dist artifacts/gnome-full-dist --only desktop-overlay-coordinates --only desktop-xregion-clip --only desktop-pulse-mixer --only desktop-xfixes-tracking --only desktop-compositor-surface --only desktop-sync-alarms --only host-lookup-reentrant --only desktop-netgroup-shadow --only gnome-introspection --report artifacts/gnome-desktop-matrix.json --timeout 45
```

坐标测试通过真实 ELF libXcomposite 创建 overlay，验证屏幕尺寸、双向 root/window 坐标转换及 XQueryPointer。其他探针检查真实裁剪像素、GLX 纹理读回、事件/计数器、异步 Pulse 回调、账户/主机查询与 GI 动态库装载。

## 当前边界

Shell 启动和这些探针通过不等于完整 GNOME 系统兼容。当前仍缺 PipeWire 屏幕采集、完整 GDM/logind/NetworkManager/bolt 服务、键盘布局上传、Pulse 录音和设备管理等后端。启动日志还存在部分服务警告及 GObject 告警；首次启动服务可能超时。加载器失败恢复保留单调分配的 TLS ID，不进行 TLS 模块编号回收。

2026-09-18 的分阶段实现、实测截图和验收报告见 `artifacts/gnome-implementation-result.md`。

## 2026-09-19 启动修复与 WebP

GNOME 依赖闭包包含 `webp-pixbuf-loader` 和真实 libwebp 解码库。安装后执行上面的 `prepare-desktop.py` 更新 GdkPixbuf 缓存，即可直接读取 WebP 壁纸。回归覆盖透明无损、有损、分段读取、截断错误和原始 4096×4096 默认壁纸。

此次修复了 fork 子进程继承 Windows SRW 等待队列指针、短时递归锁与加载器冻结竞争、已退出线程导致 fork 失败，以及缓冲的 Composite 请求晚于原生尺寸查询执行的问题。XNextRequest 同时改为返回下一条请求的序号。

```powershell
python tests/guest/tool-matrix.py --worker artifacts/gnome-full-dist/worker.exe --root artifacts/gnome-startup-root --dist artifacts/gnome-full-dist --only desktop-webp-pixbuf --only desktop-gdk-error-trap --only desktop-recursive-mutex-fork --only desktop-composite-pixmap --report artifacts/gnome-ordered-regressions.json --timeout 45
```

本轮四项客体回归、40 项 pthread 和 30 项 fork 相关原生测试通过。实际 Shell 已完成启动并绘制桌面，Settings 和 Terminal 窗口可见；这不代表上述系统服务缺口或所有鼠标交互已经验收。现场结果见 `artifacts/gnome-startup-fix-result.json`。

## 2026-09-26 启动诊断

`prepare-desktop.py` 现在还生成 GTK 图标主题缓存。缺少 `/etc/localtime` 时，根据已有 `/etc/timezone` 补齐；两个文件都缺失时使用与基础 root 一致的 `Etc/UTC`。已有自定义 `/etc/localtime` 保留；设置本地时区请显式传入 `--timezone`。新根目录还应安装 `libc-bin` 的 `C.utf8` 数据和 `tzdata`，仅安装 GNOME 的 ELF 依赖不等于已完成桌面配置。

`run-gnome.py --timezone Asia/Shanghai` 可以仅为该次会话选择已安装的时区。启动报告新增 `startup_seconds` 和 `startup_stages`，其中 Shell 就绪来自启动动画完成后的日志；原 `seconds` 仍是整个观察及清理时长。该指标不是首帧或输入验收。`--timeout 30 --stop-after-ready 3` 可在 Shell 就绪后观察 3 秒再结束；这仍会启动真实的全屏桌面，不能作为无界面测试运行。

第一轮连续图形测试影响了宿主屏幕，随后停止。用户再次授权限时测试后，第二轮改在独立的 Windows 桌面中运行，不切换输入桌面；每次使用 24 秒观察上限和独立的 27 秒 Job 清理保护。

启动器现在由 `dbus-run-session` 直接启动一份 Python，重叠 GI 导入与 system bus 启动，准备完成后用 `exec` 替换为 Shell。应用在同一会话内启动；D-Bus 自动激活服务也会获得更新后的 system bus、IBus、运行目录及桌面环境。Shell 退出后，宿主 Job 清理该会话的所有进程。

固定分发、相同测试根的六组交替测量中，Shell 启动前的中位耗时为 2.18 → 1.75 秒；完整启动中位数为 6.72 → 6.51 秒。宿主负载和文件缓存未完全受控，不能把这组数据解释为固定的提速比例或冷启动成绩。应用/D-Bus 环境、异常退出和限时清理检查通过。详见 [启动诊断记录](gnome-startup-performance-2026-09-26.md)。无图形检查：`python tools/test-gnome-startup.py -v`。

第三轮把 AccountsService 激活与 IBus 初始化重叠，并避免 Shell 已连接 IBus 后再次启动 daemon。`prepare-desktop.py` 现在为识别到的 GNOME 资源生成单文件覆盖；启动器校验原 ELF 和覆盖内容，更新或损坏时回退，显式资源覆盖优先。重新运行准备命令即可为已有 root 生成此缓存。该批次成功样本中位数为 4.03 → 3.43 秒，基线 6/7、新版 7/7 成功；不能跨批次与上面的 6.51 秒直接计算提速。详见 [第三轮瓶颈与回归记录](gnome-startup-round3-2026-09-26.md)。

第四轮把剩余路径细分为 Shell 前置启动约 1.30 秒、exec 至 Mutter 约 0.44 秒、Mutter 至 Shell 就绪约 1.71 秒，并试验 IBus 事件通知、辅助服务提前激活和已有新版运行库。没有确认新的稳定提速，生产启动器保持第三轮版本。24 次受控运行的结果、一次 IBus 提前退出及所有清理记录见 [第四轮瓶颈分析](gnome-startup-round4-2026-09-26.md)。

第五轮补齐原生 libX11 的 XIM 事件过滤接口，修复 ibus-x11 因缺少符号而装载失败、继而被当成 Shell 脚本重试的问题。真实 XIM 握手和 fork 检查通过，启动累计 Windows 进程数由 67 降至 61；重建原生分发后生效。该批完整启动中位数 4.23 → 4.13 秒，负载波动下不能认定为稳定提速；直接启动 XIM 的额外实验未合入。40 次测试全部在独立桌面限时结束并清理进程，详见 [第五轮 XIM 修复与对照](gnome-startup-round5-2026-09-26.md)。
