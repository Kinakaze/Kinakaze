# 网易云音乐 Linux 客户端运行记录：2026-09-23

在 Windows 上通过 Kinakaze 运行原版 Linux x86-64 网易云音乐 1.2.0.3。已显示完整中文 Qt/CEF 界面并验证鼠标页面切换；本地 WAV 已通过客户端输出到 Windows 音频设备。旧版在线 API 返回 HTTP 500，在线推荐、登录和在线曲库尚未验收。

## 启动

现有工作区已准备独立客体和运行时。在仓库根目录执行：

```powershell
python tools/run-netease.py
```

指定客体内的本地音乐：

```powershell
python tools/run-netease.py -- /tmp/kinakaze-audio-check.wav
```

默认运行时为 `artifacts/netease-dist`，客体为 `artifacts/netease-cloud-music/rootfs`。运行器使用持久化 `/home/netease`、独立 XDG 运行目录和 D-Bus 会话。日志写到 `artifacts/netease-cloud-music/logs`。关闭运行器会通过所属 Windows Job 清理本次启动的子进程。`--timeout 90` 可用于限时测试。

客户端启动时可能等待旧在线接口超时，首页会先显示加载状态，随后提示网络错误；这与 CEF 黑屏故障不同。

## 已修复的兼容问题

- XInput 旧设备接口、CEF 使用的 libc 分配器别名与宽字符函数、Xlib CARD32 属性长度转换。
- 被动 udev/netlink 监听端点，以及非阻塞读取、epoll、socket 选项和 fork 继承；不转发宿主热插拔事件。
- Qt XCB 与 CEF Xlib 各自拥有连接和事件队列，共用原生事件分发；读取一个连接不会吞掉另一连接的属性回复或重绘事件。
- CEF 子窗口识别 Qt 的 XCB 窗口编号，支持嵌入、坐标转换和窗口属性查询。
- CEF 构造的 `XImage` 提供有效行跨度但 `bitmap_pad=0`，现在可以正确绘制且不修改调用者的结构。
- XInput GenericEvent 的 peek 和 put-back 独立拥有 cookie 数据及嵌套指针，修复鼠标移动/点击触发的悬空指针崩溃。
- 修复 mmap 非 64 KiB 对齐地址提示与 Windows 占位区的偏移处理。
- 补全 VLC ALSA 插件所需的文本缓冲区、参数/状态转储、暂停能力和声道映射接口；使用已有 WaveOut 后端输出真实音频。
- 条件变量等待增加取消点，取消时先重新持有调用者 mutex，再运行 GNU C 清理帧，避免 VLC 在播放结束或切换曲目时一直等待线程退出。

## 客体来源与准备

原官网下载的 Ubuntu 1.2.1 包在本机返回 HTTP 403，本次使用 [Deepin 软件仓库](https://community-packages.deepin.com/deepin/pool/non-free/n/netease-cloud-music/) 的 `netease-cloud-music_1.2.0.3-1_amd64.deb`。

- 软件包 SHA-256：`5c05c0e27f2f60ecbf6dd5d4d1e842f971a0526588c861e2a55d673ab780aa6b`。
- 客户端 ELF SHA-256：`aaa055dc40a1cca6ca1f3f5871e0e82e408c12a07310afbd34a90713e08a3e8b`。
- Qt 库统一为 Deepin 5.15.1，补充对应的 OpenSSL 1.1、QCEF/CEF、VLC 3.0.12.1 音频插件及文泉驿中文字体。
- 原 CEF 资源目录保留，并将同一 `libcef.so` 字节复制到标准库目录，解决私有库目录解析问题。
- 29 个 Deepin 包的版本、大小和 SHA-256 固定在 `tools/guest-deps/netease.lock.json`。从 HTTPS 仓库下载并验证大小/散列，未单独验证 Release 签名；不执行包维护脚本。
- 最终准备记录为 `artifacts/netease-cloud-music/packages.json` 和 `dependencies.json`，包含 2,164 个文件、1,760 条 ELF 依赖边。未声称安装完整 VLC 视频功能。

重新准备依赖（需要现有 `gnome-startup-root` 提供基础配置及项目锁中未包含的支持库）：

```powershell
python tools/prepare-netease.py --support-root artifacts/gnome-startup-root
```

应用、QCEF 和 CEF 保持原包字节。诊断时临时开启过 QCEF DevTools，已恢复原库；最终运行不依赖修改第三方二进制。

## 验证记录

- `abi-r14`：MixedXlibXcbProbe、NeteaseStartupAbiProbe、MmapHintProbe、UdevMonitorProbe 全部通过。XImage 回归在旧 r12 失败、新版本通过。
- `xcb-r14`：XcbCoreProbe、XcbInputProbe、XcbDrawingProbe、XcbSelectionProbe 全部通过。
- `alsa-r15`：NeteaseAudioAbiProbe 验证文本缓冲区扩容/复用、状态转储和声道映射释放。
- `vlc-alsa-r15.log` 与对应 meter JSON：原版 libVLC 成功选择 ALSA，8 秒 WAV 的播放位置到达 7,894 ms；宿主该进程音频会话峰值约 0.00765。
- `client-audio-final.json`：网易云客户端自身输出 8 秒 WAV，记录 487 个宿主音频会话样本，峰值约 0.00392；客户端播放结束记录相符。测试音量很低，没有更改宿主全局音量。
- `cond-cancel-before`：旧版本取消正在等待条件变量的线程会超时；用于复现播放结束后的等待问题。
- `abi-r16`：新增条件变量取消/清理回归，以及上述启动、内存映射、udev、混合 Xlib/XCB 和 ALSA 回归共 7 项全部通过。`vlc-alsa-r16.log` 确认播放结束后正常退出，退出码 0。
- `xi-direct-exec.log`：以实际发布库编译/执行 XInput Rust 单元测试，通过 peek/free、put-back/free、嵌套 valuator 指针及连接隔离检查。原 Cargo 单独选包测试会混用不同 feature 集的原生 DLL，改为直接链接已发布构建的依赖后通过。
- `client-audio-r16.json`：最终客户端 606 个音频会话样本，其中 538 个非零，非零样本跨度 89.2 秒；反复播放后仍能切换本地音乐页面并暂停。最终截图为 `artifacts/netease-cloud-music/netease-running-final.png`，客户端保留打开，测试音频已暂停。

最终 `artifacts/netease-dist` 与通过上述验证的 `artifacts/netease-r16-dist` 逐文件 SHA-256 一致。构建使用隔离工作树 `artifacts/netease-build-src`，避免工作区其他正在进行的修改影响本次结果；对应修复同时保留在主工作区源码中。

在线请求日志记录服务器端 `NoClassDefFoundError: com/netease/urs/utils/SimpleCipher` 和 HTTP 500。客户端可运行及本地播放通过，不代表旧版在线服务恢复可用。
