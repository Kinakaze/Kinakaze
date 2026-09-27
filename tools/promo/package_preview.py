"""Create a local player with chapter navigation for reviewing the first cut."""
import html
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v1'
timeline=json.loads((OUT/'timeline.json').read_text(encoding='utf-8'))
names={'hook':'开场','brand':'Kinakaze','vi':'Vi / Vim','ssh':'SSH','gnome':'GNOME','java':'Java','minecraft':'Minecraft','outro':'片尾'}
seen=set();buttons=[]
for s in timeline['segments']:
    if s['scene'] in names and s['scene'] not in seen:
        seen.add(s['scene'])
        buttons.append(f'<button data-time="{s["start"]}">{names[s["scene"]]}</button>')
page='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Kinakaze · 宣传片初版</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#0b1024;color:#edf5ff;font:16px/1.7 system-ui,"Microsoft YaHei",sans-serif}
main{width:min(1200px,94vw);margin:32px auto}small{color:#90f6dd;letter-spacing:.2em}h1{font-size:clamp(24px,4vw,40px);line-height:1.3;margin:10px 0 26px}
video{display:block;width:100%;background:#080b17;border:1px solid #496071;border-radius:14px;box-shadow:0 20px 90px #0008}
nav{display:flex;gap:10px;flex-wrap:wrap;margin:20px 0}button,a{color:#a5f5e4;border:1px solid #3d6570;border-radius:9px;background:#1a293e;padding:8px 16px;text-decoration:none;cursor:pointer;font:inherit}
button:hover,a:hover{background:#284053;border-color:#fca5cd}p{color:#a7b6cb}.links{display:flex;gap:12px;flex-wrap:wrap}footer{border-top:1px solid #334156;margin-top:30px;padding-top:18px;font-size:13px;color:#8e9fb5}
</style><main><small>KINAKAZE / FIRST CUT</small><h1>你以为这是 Linux？<br>其实，它跑在 Windows 上。</h1>
<video id="movie" controls preload="metadata" poster="poster.jpg" src="kinakaze-promo-v1.mp4"></video>
<nav>'''+''.join(buttons)+'''</nav><p>约 2 分 31 秒 · 1080p · VITS 女声 · 自然提问与揭晓</p>
<div class="links"><a href="kinakaze-promo-v1.mp4" download>下载视频</a><a href="kinakaze-promo-v1.srt" download>字幕文件</a><a href="audio/narration.wav" download>独立配音</a></div>
<footer>GNOME 为启动展示；Minecraft 为主菜单和按钮交互。演示应用与依赖另行准备。<br>本片使用 AI 合成女声与原创合成器配乐。</footer></main>
<script>const v=document.querySelector('video');document.querySelectorAll('button[data-time]').forEach(b=>b.onclick=()=>{v.currentTime=Number(b.dataset.time);v.play()});</script></html>'''
(OUT/'preview.html').write_text(page,encoding='utf-8')
print(OUT/'preview.html')
