# Minecraft 白屏修复（2026-09-23）

发布包 `artifacts/minecraft-render-dist` 已验证 Minecraft Java 26.2 的实际菜单渲染、按钮响应和正常关闭。运行使用独立 demo 目录，没有读取账号凭证或已有世界。验证截图为 `artifacts/minecraft-white-debug/main-menu.png`；画面包含标题、背景、文字和菜单按钮。

后续实现：[MAP_SHARED 跨进程 futex](futex-shared-2026-09-23.md)。下文的 MAP_SHARED 未实现边界描述的是冻结的 `minecraft-render-dist` 包，不再代表后续源码。

## 原因与修复

1. X11 映射窗口时缺少 `VisibilityNotify`。线程转储显示渲染线程停在 `glfwShowWindow`，GL 跟踪只有 GLFW 初始化时的一次空缓冲区交换。该版本 [GLFW 的可见性等待循环](https://github.com/glfw/glfw/blob/9352d8fe93cd443be18157abe81f16500549aec0/src/x11_window.c#L127-L144) 在有其他待处理事件时不会进入带超时的等待，因此不会自行超时退出。补充窗口及已映射子窗口首次可见时的通知，按各连接的事件掩码分发，并补齐 Xlib/wire 转换；重复 map 不重复发送，未映射祖先下的窗口与 InputOnly 窗口不发送。
2. 解除窗口等待后，OpenAL/libstdc++ 在 `futex(word, FUTEX_WAIT, ...)` 上收到 `ENOSYS`，抛出 `std::system_error` 并终止进程。缺少 `FUTEX_PRIVATE_FLAG` 不表示地址一定来自共享映射。现在依据 Linux mmap 元数据和私有分配器所有权允许私有地址上的普通 futex，包括 JVM fork 后变成原生 section view 的私有堆。普通与 PRIVATE 操作使用独立队列键；真正 MAP_SHARED 的跨进程 futex 仍返回未实现，requeue/wake-op 同样检查目标地址。

## 验证

- `XVisibilityProbe` 在旧包中因缺少通知失败；新包验证映射、重映射、祖先可见性、多个连接的选择掩码、InputOnly 排除和 wire 状态转换通过。
- `FutexUnflaggedProbe` 在旧包中返回 ENOSYS；新包的堆、fork 后堆、MAP_PRIVATE 等待/唤醒、标志位队列隔离、bitset 超时和 MAP_SHARED 边界检查通过。
- `MixedXlibXcbProbe`、`GLStorageProbe` 通过。
- libc 的 7 项 futex 单元测试通过，覆盖超时、位集、重排队、原子 wake-op 和信号中断。
- 实际客户端约 30 秒时已显示首次欢迎界面；点击 Continue 后显示主菜单。发送 `WM_CLOSE` 后日志出现 `Stopping!`，进程退出码为 0。主菜单截图采集时已确认该窗口在前台；另两张未能取得前台的定时截图未作为画面证据。

证据位于 `artifacts/minecraft-white-debug/`：`final.log`、`main-menu.png`、`interaction.json`、`result.json`、`final-probes/results.json` 和 `futex-unit.log`。白屏基线为 `artifacts/startup6-minecraft-paired/white-before/`，后续音频崩溃的 syscall 跟踪为 `artifacts/minecraft-white-debug/futex-fixed-trace.log`。

本轮没有验收世界生成、存档和完整游玩流程。演示账号的在线服务 401、缺少 flite 旁白库及部分标准光标形状告警仍存在；它们没有阻止此次菜单渲染、按钮操作与正常关闭。VisibilityNotify 此次补充的是映射后初始通知，未实现完整的原生窗口遮挡变化跟踪。

## 构建与运行

```powershell
./tools/build.ps1 -Release -SkipFormat -SkipTests -DistDirectory artifacts/minecraft-render-dist
python tools/test-runtime-compatibility.py --root artifacts/gnome-startup-root --dist artifacts/minecraft-render-dist --output-dir artifacts/minecraft-white-debug/final-probes --probe XVisibilityProbe --probe FutexUnflaggedProbe --probe MixedXlibXcbProbe --probe GLStorageProbe
python tools/run-minecraft.py --root artifacts/guest-root --dist artifacts/minecraft-render-dist --worker artifacts/minecraft-render-dist/worker.exe
```

构建时未运行整仓库格式及全量测试；上述功能回归和 futex 单元测试单独执行。旧的冻结发布包保留作为基线，启动修复版需使用这里的 `--dist` 和 `--worker`。
