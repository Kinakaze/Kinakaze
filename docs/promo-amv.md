# Kinakaze · 你好，Windows。

## 当前版本：片尾与配音调整 v4

成片：`artifacts/promo-amv/kinakaze-amv-v4-2k.mp4`，53.77 秒，2560 × 1440 / 60 fps。

- 44.17 秒后的角色与功能快切改为一段连续角色近景，取消跳动边框与片尾闪白。
- Minecraft 录屏继续播放，并在 0.8 秒内叠化到角色近景；49.92 秒起再以 0.8 秒叠化进入项目名和开源地址，近景运动在接缝处连续。
- “你好，Windows”保留画面文字，移除这句配音及对应音乐压低。沿用已选 A 女声的其他三句，后半段旁白时间不变。
- 片尾不再附加切镜提示音。CLI 演示与其他内容沿用 v3。

`amv-settings.json` 默认指向 v4。v3 成片保留，修改前的渲染源文件、时间线、混音和预览页保存在 `artifacts/promo-amv/archive-v3`。

导出检查通过：3226 帧、完整解码无错误、音频偏移 0 毫秒、约 -13.91 LUFS，9 个章节跳转与移动端预览正常。抽查成片关键帧，并检查片尾 580 帧的亮度曲线，未发现孤立闪帧；开场原旁白位置的混音与纯背景音乐一致。报告：`reports/validation-v4.json`、`preview-v4.json`、`edit-v4.json`。

## CLI 演示精修 v3

成片：`artifacts/promo-amv/kinakaze-amv-v3-cli-2k.mp4`，53.77 秒，2560 × 1440 / 60 fps。用户明确本轮重点是 CLI 体验。角色、已选 A 女声、音乐和无眨眼设定延续；演示中心改为手动命令行工作流。

- vi / Vim 各给出约 3.84 秒，连续展示输入、Esc、`:wq` 保存和 `cat` 读回。沿用同一窗口，减少重复入场。
- 终端字号提高，光标按等宽字体的 `ch` 单位对齐；裁切无关空行，把保存与读回输出移到上方。状态栏和键位提示保留在固定位置。
- CLI 段约 7.68 秒，实际执行 `cat tasks.txt` → `cat tasks.txt | sort -u > result.txt` → `cat result.txt`。输入文件有重复行，结果去重排序，退出码为 0，并在 Windows 侧读回验证。证据：`reports/cli-demo-v3.json`。
- 命令按实际输出重新排版，输入速度、镜头停留与 125 BPM 音乐对齐。它不是连续原速录屏。vi/Vim 的原始终端素材通过 SSH TTY 采集；管道命令通过 worker CLI 直接执行。
- SSH、Java、GNOME 和 Minecraft 保留真实运行素材。GNOME 镜头展示已有运行状态；Minecraft 仍只展示主菜单。
- 旁白的文件读回句移至管道执行后半段；各段输出留出阅读时间。移除了反复扫过终端窗口的白色转场。

| 时间约 | 内容 |
| --- | --- |
| 0–11.52 秒 | 角色、起势与项目登场 |
| 11.52–15.36 秒 | vi 输入、Esc、`:wq` |
| 15.36–19.20 秒 | Vim 追加、保存、读回 |
| 19.20–23.04 秒 | SSH / sshd |
| 23.04–26.88 秒 | GNOME |
| 26.88–34.56 秒 | CLI 管道、重定向、读回 |
| 34.56–38.40 秒 | Linux Java |
| 38.40–44.16 秒 | Minecraft Java 主菜单 |
| 44.16–49.92 秒 | 角色与功能卡点回切 |
| 49.92–53.77 秒 | 项目与开源地址 |

新增 `src/amv-demo.tsx` / `amv-demo.css` 管理终端演示。复现前执行 `python tools/promo/lab/record_cli_demo.py`，其余渲染步骤见下方。校验报告与预览检查按版本保存。v2 成片及 `archive-v2` 中的源文件、时间线保留。

导出检查已通过：3226 帧、2560 × 1440 / 60 fps、完整解码无错误，成片音频相对 PCM 混音测得 0 毫秒偏移，约 -13.96 LUFS。9 个章节跳转和手机宽度预览检查通过。报告：`artifacts/promo-amv/reports/validation-v3.json` 与 `preview-v3.json`。回车/保存使用轻音效，实际时间保存在 `timeline.json` 的 `ui_feedback` 中。

## v2 设计记录

这一版为角色主导的 AMV 节奏宣传短片，42.23 秒，默认 **2560 × 1440 / 60 fps**。沿用蓝白角色、音乐卡点与程序演示。

精修版成片：`artifacts/promo-amv/kinakaze-amv-v2-2k.mp4`。旧版 `kinakaze-amv-v1.mp4` 保留。

第二版去掉全部眨眼切帧、文字与窗口的模糊入场，减小近景放大及入场位移；删除泛泛的抒情口号，将文案改为直接说明功能。四句配音重新合成，继续使用选定的 A 女声。主要信息为“在 Windows 上运行 Linux 程序”“SSH 远程连接”“接入 Agent 工具调用”“现已开源”。

## 角色

采用用户选定的原创银白发少女方向，并按随后反馈重新设计为娇小、圆润面部、蓝白配色与可爱的层叠白裙。第一张偏成熟设计弃用。

当前角色素材：

- `artifacts/promo-amv/public/art/hero.png`：正面立绘。
- `artifacts/promo-amv/public/art/action.png`：伸手打开界面的动作。
- `artifacts/promo-amv/public/art/close-hq.png`：重新细化线条的睁眼近景，1672 × 941。

原 `close.png` 与 `blink.png` 仅作旧版归档，第二版不加载闭眼帧。

使用内置 **imagegen** 生成和编辑。提示词保存在 `artifacts/promo-amv/reports/image-prompts.json` 与 `image-prompts-v2.json`。人物与衣服为原创，没有使用现有动画片段。

本版角色运动由立绘网格变形、轻微头发与裙摆摆动、镜头运动和换姿剪辑组成，不含眨眼。它是动态立绘的 AMV 剪辑，不是逐帧绘制的完整角色动作动画。

## 音乐驱动的剪辑

使用 [しゅわしゅわハニーレモン350ml / しゃろう](https://opentracks.com/bgm/detail/14621)，作曲者将其描述为四拍节奏的 Kawaii Future Bass。音源来自该页官方下载入口；使用依据为 [OpenTracks 音源许可](https://opentracks.com/help/articles/license/)。

对下载的实际音频进行低频及高频正向频谱变化分析，得到 **125 BPM**，每拍 0.48 秒。谱峰相位为 0.025 秒。剪辑取第 80 拍附近开始，先保留四小节起势，**7.68 秒**进入主段。镜头边界统一按拍数计算并舍入到 60 fps 的最近帧。

旁白沿用选定的 A 女声，新合成四句。第一段主重拍前结束旁白，使重拍登场保留完整音乐；音乐主导其余展示，只在另外两句人声出现时做短时间避让。混音目标 -14 LUFS。

| 时间约 | 内容 | 宣传目的 |
| --- | --- | --- |
| 0–5.76 秒 | 近景、白裙少女、伸手打开界面 | 建立项目角色与“你好，Windows”主题 |
| 5.76–7.68 秒 | 半拍快速字切 | 音乐起势，连接 Linux、Windows、Agent |
| 7.68–11.52 秒 | 品牌登场 | 说明 Kinakaze 的核心用途 |
| 11.52–19.20 秒 | vi、Vim、SSH / sshd | 熟悉工具的真实操作与连接结果 |
| 19.20–23.04 秒 | GNOME 动态实录 | 从终端展开到桌面 |
| 23.04–26.88 秒 | Agent 工具调用 | 命令、输出、退出码与文件读回 |
| 26.88–28.80 秒 | Linux Java | 实际运行输出 |
| 28.80–32.64 秒 | Minecraft 主菜单 | 熟悉应用的揭晓 |
| 32.64–38.40 秒 | 角色近景与功能交叉快切 | 音乐主段收束，强化记忆 |
| 38.40–42.23 秒 | 项目名、角色、仓库地址 | 开源使用邀请 |

终端使用已录制的真实屏幕状态重新排版，按时间加速播放；vi 保留编辑片段，Vim 展示保存与读回。GNOME 与 Minecraft 为真实动态录屏。Agent 为 CLI 工具适配示例，不声称内置 MCP 服务；Minecraft 镜头仅表示客户端主菜单，不表示世界内游玩已验证。

文字、矢量元素及 Three.js 的实际绘图缓冲区均按 2560 × 1440 输出；中间帧采用 PNG，H.264 CRF 15。录屏源为 1920 × 1080，角色原画也有自己的源分辨率；2K 是合成输出规格，不代表全部素材原生 2K。近景减少了放大，录屏取消额外缩放。Minecraft 原 720p 素材换为完整 1080p 重录，避免超出物理屏幕导致裁切。

## 工程

- `tools/promo/lab/src/amv.tsx`、`amv.css`：角色动态、镜头、功能演示、转场。
- `tools/promo/lab/amv_music.py`：下载正确音源并测量节奏。旧 DOVA 编号和 OpenTracks 编号并不完全一致，脚本会检查曲名。
- `tools/promo/lab/amv_prepare.py`：时间线、素材、音频混合。
- `tools/promo/lab/amv-voice.json`：当前三句配音文案。
- `tools/promo/lab/render-amv.mjs`：Remotion 渲染入口。
- `tools/promo/lab/amv-settings.json`：默认输出分辨率、帧率、质量及文件名；渲染与校验共用。
- `tools/promo/lab/recapture_minecraft_2k.py`：在物理屏幕范围内录制完整 1080p Minecraft 窗口，供 2K 合成使用。
- `artifacts/promo-amv/timeline.json`：逐拍、逐镜头及人声的时间点。

```powershell
python tools/promo/lab/amv_music.py
& E:/APPD/VITS/VITS-barbara/env/python.exe F:/crysoacu2/tools/promo/synthesize.py --speaker 1 --speed 1.15 --config F:/crysoacu2/artifacts/promo-v2/assets/voice-config.json --checkpoint F:/crysoacu2/artifacts/promo-v2/assets/voice-model.pth --script F:/crysoacu2/tools/promo/lab/amv-voice.json --output F:/crysoacu2/artifacts/promo-amv/voices
& E:/APPD/VITS/VITS-barbara/env/python.exe tools/promo/lab/amv_prepare.py
Push-Location tools/promo/lab
node render-amv.mjs stills
node render-amv.mjs test
node render-amv.mjs full
Pop-Location
python tools/promo/lab/amv_finish.py
python tools/promo/lab/amv_preview_check.py
```

生成的图片须保留在项目 `public/art` 目录。仅下载音乐并执行上述命令不会重建原画，原画提示词和内置工具来源另有记录。

## 导出检查

`amv_finish.py` 从 PCM 混音重新封装音频，避免 Remotion 中间音频产生的约 42.7 毫秒延迟，视频保持原编码。检查包含 2534 帧、1440p60、实际 Three.js 绘图缓冲区、完整解码、音量、音画同步及多段画面变化。第二版结果保存在 `artifacts/promo-amv/reports/validation-v2.json`，`validation.json` 为最新版本；旧版结果另存为 `validation-v1.json`。`index.html` 提供带章节跳转的本地预览。
