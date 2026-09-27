# Minecraft 视角不动：原生焦点同步

## 定位

Minecraft 使用的 LWJGL 3.4.1 GLFW 原生库，在普通光标模式下有鼠标位移回调；切到 `GLFW_CURSOR_DISABLED` 后，原始输入开启和关闭时均没有位移回调。

旧包 `artifacts/release-v0.1.0` 的最小复现记录位于 `artifacts/mouse-debug/probe-release-v0.1.0.log`。日志先收到 `FOCUS 1`，但随后两个阶段的 `glfwGetWindowAttrib(GLFW_FOCUSED)` 均返回 0。这与键盘和菜单操作正常、进入世界后视角不动的表现一致。

原因是 `WM_SETFOCUS` / `WM_KILLFOCUS` 只转换为 X11 焦点事件，没有更新 `XGetInputFocus` 使用的状态。GLFW 在启用鼠标捕获前会再次查询焦点，查询失败后不会设置控制相对位移的窗口。因此关闭原始输入也无法绕过这处问题。相关行为可见 [GLFW X11 光标模式实现](https://github.com/glfw/glfw/blob/9352d8fe93cd443be18157abe81f16500549aec0/src/x11_window.c#L2674-L2708)。

## 修改

在分发原生焦点事件之前，同步本地焦点与会话共享焦点记录，使查询、键盘路由及 FocusIn / FocusOut 一致。保留显式 `XSetInputFocus` 的回退策略；旧窗口迟到的失焦事件以及比当前状态更旧的事件不能覆盖新焦点。

状态转换逻辑在 `libs/libX11/src/focus/native.rs` 中，可独立测试，不需要创建窗口或注入鼠标操作。另有 `focus/tests.rs` 中的事件桥接集成测试。

## 验证与边界

- 不创建窗口的 5 项状态转换测试通过，覆盖激活与失焦、跨窗口事件顺序、显式焦点回退策略、32 位时钟回绕及共享记录布局。
- Release 库构建和独立打包成功；29 个模块、5785 个客体导出的校验通过。修复包运行 BusyBox 命令行探针输出 `MINECRAFT_MOUSE_DIST_OK`，退出码为 0。Minecraft 启动元数据与依赖哈希校验通过，未启动游戏。
- 旧包使用 Minecraft 同一份 `libglfw.so` 的复现已完成：普通鼠标回调正常，禁用光标后的两种输入模式均无回调。
- 图形验证曾干扰正在使用的桌面；收到反馈后停止所有桌面操作，确认测试进程退出并解除鼠标限制。修复后没有再次自动运行 GLFW 或 Minecraft，因此尚无修复后世界内游玩验收。
- X11 / XI 完整单元测试曾因混合构建产物的 Rust DLL 导入不匹配而无法启动，不能算作通过。发布包改为在完整工作区库构建结束后，从独立目录收集同一构建的产物。

无窗口测试命令：

```powershell
rustc --edition 2024 --test libs/libX11/src/focus/native.rs -o artifacts/mouse-debug/native-focus-tests.exe
./artifacts/mouse-debug/native-focus-tests.exe --test-threads=1
```

修复包的启动入口是 `artifacts/minecraft-mouse-dist/run-minecraft.cmd`，使用本地 `artifacts/promo-v1/guest-root` 中安装的客户端，以 `--full` 启动完整本地游戏模式，不传入 `--demo`。游戏目录为 `minecraft/v2-game`。启动脚本默认也是完整本地模式，显式指定 `--demo` 时使用原来的 `minecraft/v2-demo`。当前使用本地离线身份，尚未接入账号登录。也可以使用自己的 root：

```powershell
python tools/run-minecraft.py --full --root <你的客体目录> --dist artifacts/minecraft-mouse-dist --worker artifacts/minecraft-mouse-dist/worker.exe
```

构建与打包日志保留在 `artifacts/mouse-debug/final-build.log` 和 `package.log`。
