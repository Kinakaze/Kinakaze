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
python tools/prepare-desktop.py --dist artifacts/gnome-full-dist --root artifacts/gnome-startup-root
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
