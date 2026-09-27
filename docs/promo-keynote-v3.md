# Kinakaze 宣传片 · 发布会动效版

更新：2026-09-26。当前版本为 v3。v1 的深色配色与合成配乐已放弃；v2 仅用于确定清透配色。v3 按最新反馈重新组织为连续 3D 动画与真实软件演示。

## 标题与方向

**给 Windows 上的 Agent，接上 Linux 工具｜Kinakaze**

面向 B 站，以 Agent 调用 Linux 工具的场景开场。一份文件展开成目录，路径在空间中生长，穿过 Kinakaze 核心后展开为真实的软件画面。GNOME、Minecraft 保留自然悬念与揭晓；不使用倒数、吐槽、人物或看板娘。

雾白、冰蓝、玻璃与浅色金属；真实三维透视、物理材质、环境反射、阴影、连续运镜、曲面展开、体素生长与跟随节拍的二维文字。软件演示占主要画面。输出为 1920×1080、60 fps、127.9 秒，软件录屏源为 24 fps。

## 声音

用户明确选择试听 **A**。使用本地 VITS 推理，模型 `guetLzy/VITS-fast-fine-tuning`，speaker 1，速度 1.15。声音为 AI 合成，无真人录制或人物出镜。

配乐为 **SUMMER TRIANGLE / しゃろう（Sharou）**，替换 v1 的脚本合成伴奏。通过官方素材页取得音源，保留来源与 SHA-256。对白时压低背景音乐，留白与揭晓时抬起。混音以 -16 LUFS、-1.5 dBTP 为处理目标，导出后的实际测量见检查报告。

- [B 站音乐参考](https://www.bilibili.com/video/BV1gZ4y1V7kx/)
- [Pixel Galaxy 调研参考](https://www.bilibili.com/video/BV19x411773R/)：未使用这首曲目的音频或画面。
- [官方曲目](https://opentracks.com/bgm/detail/12983)、[素材使用规则](https://opentracks.com/help/articles/license/)、[作者页面](https://opentracks.com/creator/detail/101)
- [VITS 模型](https://huggingface.co/guetLzy/VITS-fast-fine-tuning)、[B 站模型介绍](https://www.bilibili.com/video/BV1b84y1U7yv/)

## 时间线

| 时间 | 内容与动态 |
| --- | --- |
| 0–7 秒 | 文件展开，工具路径从中心生长，引出 Agent。 |
| 7–16 秒 | 三种路线以环境层、远程连接、工具齿轮表达。 |
| 16–29 秒 | 路径汇入玻璃核心，镜头靠近，揭晓 Kinakaze。 |
| 29–44 秒 | Agent → CLI → Linux 工具 → 本地文件；展示实测输出与退出码。 |
| 44–62 秒 | Vi、Vim、SSH 真实终端记录，曲面展开与移动镜头衔接。 |
| 62–78 秒 | “不止命令行”揭晓 GNOME，两段启动视图交叉过渡。 |
| 78–89 秒 | Linux Java：版本、JIT、线程、文件、子进程。 |
| 89–111 秒 | 体素波浪引出 Minecraft，真实 Continue 点击与动态菜单。 |
| 111–128 秒 | 路径重新汇聚，强调 Agent 执行路径、用户态与开源。 |

## 宣传口径与实录依据

新增 `tools/promo/record_agent_demo.py`，真实调用冻结版本 `worker.exe run` 执行 Linux BusyBox sort，取得 stdout、退出码，并由 Windows 侧读回客体目录中的同一个输出文件。程序断言结果一致。报告：`artifacts/promo-v3/reports/agent-demo.json`。

这支持“把 CLI 入口包装为 Agent 工具”的表达。镜头标为“CLI 工具调用示例”，未宣称内置 MCP、特定 Agent 官方集成或任意宿主目录自动挂载。共享文件范围为准备好的客体目录 `/workspace/promo-agent/`。

比较针对使用路线：WSL 完整环境的维护；远程 SSH 的连接与文件同步；MSYS2 的 Windows 移植工具定位。没有速度、内存、安装体积排名，也未声称 WSL/MSYS2 完全无法共享 Windows 文件。

依据：[微软文件系统说明](https://learn.microsoft.com/en-us/windows/wsl/filesystems)、[WSL 互操作](https://learn.microsoft.com/en-us/windows/dev-environment/wsl-interop)、[MSYS2 定位](https://www.msys2.org/docs/what-is-msys2/)。

Vi/Vim/SSH/Java 使用 v1 的真实 ConPTY 输出，按时间戳解析。剪去登录和打字前奏，剪辑速度不代表性能。GNOME 仅展示启动后的桌面与 Settings；原始启动录像前 4.5 秒的非目标画面已去除，镜头运动属于后期。Minecraft 展示菜单背景与 Continue 响应，世界内游玩仍未验收。运行时固定为 `artifacts/release-v0.1.0`，不改动其他开发任务的构建。

## 源文件与输出

- `tools/promo/script-v3.json`：时间线、旁白、发音稿。
- `tools/promo/keynote.html`、`keynote.js`：三维场景、材质、生长动画、二维排版、逐帧时间控制。
- `tools/promo/capture_keynote.py`：本地素材服务、确定性 WebGL 帧导出、封装；定期释放 GPU 捕获资源，时间戳保证重建后连续。
- `tools/promo/prepare_v3.py`：终端状态、干净录屏、字幕、混音。
- `tools/promo/verify_v3.py`：完整解码、音量、画面变化、浏览器错误、演示依据。
- `artifacts/promo-v3/kinakaze-promo-v3.mp4`：成片。
- `artifacts/promo-v3/preview.html`：章节播放器。
- `artifacts/promo-v3/kinakaze-promo-v3.srt`：字幕。
- `artifacts/promo-v3/reports/video-validation.json`：导出检查。

## 重建

保留 v1 的录屏/终端记录、v2 的声音模型与音乐、v3 的 Three.js 0.186.1 本地依赖。

```powershell
python tools/promo/record_agent_demo.py
& 'E:/APPD/VITS/VITS-barbara/env/python.exe' tools/promo/synthesize.py --speed 1.15 --speaker 1 --config F:/crysoacu2/artifacts/promo-v2/assets/voice-config.json --checkpoint F:/crysoacu2/artifacts/promo-v2/assets/voice-model.pth --script F:/crysoacu2/tools/promo/script-v3.json --output F:/crysoacu2/artifacts/promo-v3/audio
python tools/promo/prepare_v3.py
python tools/promo/capture_keynote.py --preview
python tools/promo/capture_keynote.py
python tools/promo/verify_v3.py
python tools/promo/package_v3.py
```

素材、模型与导出物位于 Git 忽略的 `artifacts/`，不随源码提交，不单独重新分发外部音乐。

## 发布简介

给 Windows 上的 Agent，接上 Linux 工具。

认识 Kinakaze：在 Windows x86-64 用户态运行 Linux x86-64 ELF 程序的实验性兼容层。从 CLI 工具调用和本地文件读回，到 Vi、Vim、SSH、GNOME 启动、Linux Java，再到 Minecraft 主菜单。

持续开发，欢迎体验与参与。Agent 演示通过 CLI 工具适配；GNOME 为启动展示，Minecraft 世界内游玩仍在推进。

项目：https://github.com/Kinakaze/Kinakaze

Music: SUMMER TRIANGLE / しゃろう（Sharou），OpenTracks（旧 DOVA-SYNDROME）。VITS AI 合成配音，模型来自 guetLzy/VITS-fast-fine-tuning，speaker 1。
