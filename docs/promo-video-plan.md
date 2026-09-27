# Kinakaze 宣传片初版

> 此页保留 v1 制作记录。最新方案与交付已调整为 [v3 发布会动效版](promo-keynote-v3.md)：雾白冰蓝、3D 生长、Agent 接入演示、用户选择的 A 声线，以及《SUMMER TRIANGLE》配乐。

更新：2026-09-26。以最新反馈为准：女声 VITS 配音，偏二次元，以动态软件素材为主；保留自然提问与揭晓，取消倒数、吐槽、拟人对话和看板娘。

## 标题与方向

**你以为这是 Linux？其实，它跑在 Windows 上。｜Kinakaze**

通过 GNOME 画面引出“它运行在哪里”，揭晓 Windows 与 Kinakaze，再展示 Vi、Vim、SSH/sshd、GNOME、Linux Java、Minecraft。突出项目名字、运行效果、开源与继续开发。

深蓝、薄荷绿、淡粉色，搭配几何动态图形、流动线条和轻微镜头运动。二次元气质通过女声、原创电子配乐与节奏表现，没有人物形象。

片尾：**Kinakaze。让熟悉的程序，遇见新的可能。**

## 源文件与输出

- `tools/promo/script.json`：最终旁白、字幕、语音读音稿、停顿。
- `docs/promo-voiceover.txt`：完整旁白正文。
- `tools/promo/synthesize.py`：VITS 逐段配音。
- `tools/promo/render.py`：时间线、动态图形、原创配乐、混音与导出。
- `artifacts/promo-v1/kinakaze-promo-v1.mp4`：1080p、24 fps 初版，约 2 分 31 秒。
- `artifacts/promo-v1/kinakaze-promo-v1.srt`：独立字幕。
- `artifacts/promo-v1/audio/narration.wav`：女声配音。
- `artifacts/promo-v1/audio/original-music.wav`：原创器乐底乐。
- `artifacts/promo-v1/timeline.json`：实际镜头与配音时间线。

## 本次素材

| 内容 | 录制方式与范围 |
| --- | --- |
| Vi / Vim | Kinakaze 中的 BusyBox Vi 与 vim.tiny，经真实 SSH 终端编辑、保存文件，将带时间戳的 ConPTY 输出动态回放。 |
| SSH / sshd | Windows OpenSSH 连接 Kinakaze 中的 Linux sshd；真实密钥认证、远程命令与管道，片内标明两端。 |
| Java | Linux Java 25，执行 JavaRuntimeProbe，得到 JIT、线程、文件和子进程通过标记。 |
| GNOME | 独立 Debian 客体中取得桌面、设置窗口启动画面及 Shell 出画过程。未取得有效交互变化，仅作启动展示并添加镜头运动。 |
| Minecraft | 独立 demo 目录启动 Linux 客户端，记录动态背景、Continue 点击与主菜单；未使用已有账号、世界或存档。 |

GNOME 与 Minecraft 采集独立窗口，终端按真实时间戳事件回放。剪辑时长不作为性能证据。运行时为 `artifacts/release-v0.1.0`，演示客体位于 `artifacts/promo-v1/`。

## 配音与检查

使用本机 `E:/APPD/VITS/VITS-barbara/pretrained_models/G_0.pth`、`configs/finetune_speaker.json` 的女声（speaker 0）。早期微调模型的咬字问题较多，未用于最终旁白。哈希与参数见 `audio/manifest.json`。

字幕保留正式名称，语音输入单独处理读音。Kinakaze 暂按“基纳卡泽”试读，不作为正式中文名。视频标明 AI 合成声音。

检查包括音轨与峰值、编码与时长、字幕范围、抽帧和原始录屏变化。语音识别仅作抽样辅助，不代表每个外文词的发音已达到最终发布标准。

## 重建

保留录屏素材后，在仓库根目录执行：

```powershell
& 'E:\APPD\VITS\VITS-barbara\env\python.exe' tools/promo/synthesize.py --speed 1.24
python tools/promo/render.py --preview
python tools/promo/render.py
```

工具依赖当前 Windows 环境。原始素材、日志、依赖和视频位于 Git 忽略的 `artifacts/promo-v1/`，不随源码提交。

工具参考：[PyWinpty](https://github.com/andfoy/pywinpty)、[pyte](https://pyte.readthedocs.io/en/latest/tutorial.html)。VITS 本地说明见 `E:/APPD/VITS/VITS-barbara/README_ZH.md`。配乐由脚本合成，没有使用外部歌曲录音。

## 发布简介

看起来熟悉的 Linux 桌面，这一次运行在 Windows 上。

从 Vi、Vim、SSH，到 GNOME、Linux Java，再到 Minecraft 主菜单，认识 Kinakaze：一个在 Windows x86-64 上运行 Linux x86-64 程序的实验性兼容层。

项目开源，持续开发。演示应用与依赖另行准备。GNOME 展示启动画面；Minecraft 展示主菜单与按钮响应，世界内游玩待验收。

项目：https://github.com/Kinakaze/Kinakaze

本片使用 VITS AI 女声配音。
