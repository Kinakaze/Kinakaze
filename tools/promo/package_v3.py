"""Package the completed keynote edit and its chapter navigation."""
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v3'
data=json.loads((OUT/'timeline.json').read_text(encoding='utf-8'))
labels={'seed':'开场','routes':'不同路线','hero':'Kinakaze','agent':'Agent 接入','roundtrip':'文件往返','vi':'Vi / Vim','ssh':'SSH','gnome':'GNOME','java':'Java','minecraft':'Minecraft','end':'片尾'}
seen=set();buttons=[]
for s in data['segments']:
    name=s['scene']
    if name in labels and name not in seen:
        seen.add(name);buttons.append(f'<button data-time="{s["start"]}">{labels[name]}</button>')
page='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Kinakaze / 发布会动效版</title>
<style>*{box-sizing:border-box}body{margin:0;color:#263f51;background:linear-gradient(145deg,#fff,#edf5fa);font:16px/1.75 system-ui,"Microsoft YaHei",sans-serif}main{width:min(1300px,93vw);margin:48px auto}small{color:#7e9caf;letter-spacing:.2em}h1{font-size:clamp(27px,3.8vw,47px);line-height:1.4;font-weight:500;margin:12px 0 28px}video{display:block;width:100%;border-radius:4px;box-shadow:0 25px 90px #31537018}nav{display:flex;flex-wrap:wrap;gap:8px;margin:26px 0}button{color:#52758e;background:transparent;border:1px solid #ccdfe9;border-radius:4px;padding:8px 14px;font:inherit;cursor:pointer}button:hover{background:#dfedf5}p{color:#7b92a2}.links{display:flex;gap:25px;margin:24px 0}a{color:#3d87b5;text-decoration:none;border-bottom:1px solid #bbd6e7;padding-bottom:3px}footer{border-top:1px solid #d8e5ed;margin-top:35px;padding-top:25px;font-size:13px;color:#7d929f}footer a{color:inherit}details{margin-top:16px}</style>
<main><small>KINAKAZE / MOTION FILM</small><h1>给 Windows 上的 Agent，<br>接上 Linux 工具。</h1><video controls preload="metadata" poster="poster.jpg" src="kinakaze-promo-v3.mp4"></video><nav>'''+''.join(buttons)+'''</nav><p>2 分 8 秒 · 1080p / 60 fps · 3D 生长与连续运镜 · A 声线</p><div class="links"><a href="kinakaze-promo-v3.mp4" download>下载视频</a><a href="kinakaze-promo-v3.srt" download>下载字幕</a><a href="reports/video-validation.json">查看检查结果</a></div><footer>Music: <a href="https://opentracks.com/bgm/detail/12983">SUMMER TRIANGLE / しゃろう</a> · VITS AI 合成女声<br>3D 动效与排版为本片制作，终端与应用内容来自实际运行。
<details><summary>演示范围与制作来源</summary><p>Agent 通过命令行工具适配器调用；已验证输出、退出码与准备好的客体目录中的文件读回。GNOME 展示启动画面，Minecraft 展示主菜单与按钮交互。未宣称完整 Linux 兼容、内置 MCP 服务或游戏世界内游玩。</p><p>Voice model: <a href="https://huggingface.co/guetLzy/VITS-fast-fine-tuning">guetLzy / VITS-fast-fine-tuning</a>, speaker 1. 音色与语速按用户选择的试听 A 使用。</p></details></footer></main><script>const v=document.querySelector('video');document.querySelectorAll('[data-time]').forEach(b=>b.onclick=()=>{v.currentTime=+b.dataset.time;v.play()})</script></html>'''
(OUT/'preview.html').write_text(page,encoding='utf-8')
print(OUT/'preview.html')
