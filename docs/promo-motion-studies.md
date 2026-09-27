# Kinakaze · 四种空间叙事

四支独立的宣传动效样片，每支 824 帧，13.733 秒，1920 × 1080，60 fps。目标是比较镜头、材质和运动方式，供后续完整宣传片选用。

输出目录：`artifacts/promo-lab/`。打开 `index.html` 可分别带声音观看，也可静音同步比较；`four-studies.mp4` 为四支连续播放版。

| 样片 | 画面方向 | 文案与内容 | 成片 |
| --- | --- | --- | --- |
| A · 生长之庭 | 镜头从局部后退；齿轮、塔层、花瓣依次展开；路径延伸和光点持续运动 | 从一行命令，长出更多可能。出现 vi / vim / ssh | `A-bloom.mp4` |
| B · 冰蓝切面 | Blender 微距摄影；珐琅、金属、树脂切片；过渡到 Three.js 全景与结构展开 | 把熟悉的工具，放进新的可能。 | `B-prism.mp4` |
| C · 穿行之间 | 镜头穿过环形建筑；数据流前行；尾段转入全景品牌画面 | 你的智能体，现在多了一条路。展示 Agent → CLI → Linux 和实际 CLI 输出 | `C-flow.mp4` |
| D · 世界初生 | 体素由中心向外生长；波浪、悬浮轨道；像素遮罩揭晓动态实录 | 再往前一步，会遇见什么？揭晓 Minecraft Linux 客户端主菜单 | `D-voxel.mp4` |

## 三种工具的实际分工

- **Blender 5.2.1**：原创几何建模、倒角、材质与场景文件，导出四组 GLB；Cycles 生成材质检查图。B 的开场是实际 Eevee 渲染的 124 帧微距镜头，30 fps，合成时放入 60 fps 时间线。
- **Three.js 0.186.1 / React Three Fiber**：PBR 光照和环境反射、相机、模型生长、路径、粒子、切片与体素运动。
- **Remotion 4.0.529**：固定帧时间线、逐字排版、媒体遮罩、转场、实录及音频合成，导出 H.264 / AAC MP4。

动画由当前帧推导，没有依赖实时动画循环。GLB 在 ThreeCanvas 挂载之前加载，字体也等待完成后才抓帧，保证离线渲染与预览一致。

`blender/` 保存四份建模场景及微距场景；微距相机和切片动画由 `render_blender_macro.py` 驱动。完整 Three.js 动画保存在 `tools/promo/lab/src/study.tsx`。

## 声音与素材

沿用已经选定的 **A 女声**，VITS speaker 1，速度 1.15。每支一句，新合成的音频及模型哈希记录在 `voices/manifest.json`。未引入看板娘。

音乐使用 [SUMMER TRIANGLE / しゃろう](https://opentracks.com/bgm/detail/12983)，按 175 BPM 对齐四段不同的音乐区间；加入自行合成的轻微扫频音效，并在配音出现时压低音乐。最终混音目标为 -16 LUFS。

Minecraft 使用已录制的 Linux 客户端动态主菜单，**没有声称完成了世界内游玩验证**。Agent 画面展示既有 CLI 工具调用的实际输出，不表示项目内置 MCP 服务。四支样片未加入未经验证的 WSL / SSH / MSYS2 性能比较。

## 复现

以下命令在仓库根目录运行；Blender、Chrome 和 VITS 路径对应本工作站，可按安装位置修改。

```powershell
& E:\APPD\blender\blender.exe -b --python tools/promo/lab/build_assets.py
& E:\APPD\blender\blender.exe -b --python tools/promo/lab/render_blender_macro.py
ffmpeg -y -framerate 30 -i artifacts/promo-lab/render/blender-macro/%04d.png -vf hqdn3d=1.2:1.2:2:2 -c:v h264_nvenc -preset p7 -cq 16 -pix_fmt yuv420p -movflags +faststart artifacts/promo-lab/public/blender-macro.mp4

& E:\APPD\VITS\VITS-barbara\env\python.exe tools/promo/synthesize.py --speaker 1 --speed 1.15 --config artifacts/promo-v2/assets/voice-config.json --checkpoint artifacts/promo-v2/assets/voice-model.pth --script tools/promo/lab/voice-script.json --output artifacts/promo-lab/voices
& E:\APPD\VITS\VITS-barbara\env\python.exe tools/promo/lab/prepare_media.py

Push-Location tools/promo/lab
npm ci --registry=https://registry.npmjs.org
node node_modules/typescript/bin/tsc --noEmit
node render.mjs stills
node render.mjs full
Pop-Location

python tools/promo/lab/verify_samples.py
python tools/promo/lab/package_samples.py
```

`node render.mjs full A` 可只导出 A；B / C / D 同理。`test A` 只输出前 180 帧，并覆盖同名 A 文件，之后需要重新运行 `full A`。

`prepare_media.py` 复用 `artifacts/promo-v3/assets/` 中已经准备好的音乐与 Minecraft 实录，使用含 SciPy 的 VITS Python 环境。

## 检查与交付

- `reports/remotion-full-all.json`：渲染完成情况、实际 GPU、浏览器错误。
- `reports/validation.json`：四支视频逐帧解码、尺寸、帧率、帧数、时长、音量，以及各段画面变化检查。
- `reports/audio.json`：配音时刻和音乐取段位置。
- `reports/source-hashes.json`：制作脚本与工程文件哈希。
- `stills/`：各片 1.5 / 4.5 / 7.5 / 11.667 秒关键帧。
- `credits.json`：素材与工具署名。

画面变化检查用于发现冻结画面；不能替代对构图、遮挡和转场的人工检查。完整导出后需要结合关键帧、转场抽帧和本地播放检查。
