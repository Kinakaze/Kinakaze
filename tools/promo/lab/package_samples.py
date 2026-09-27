"""Package the four samples into a local screening room and a comparison reel."""
import json,subprocess,hashlib
from pathlib import Path
from PIL import Image,ImageDraw,ImageFont
ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-lab'
studies=[('A-bloom','生长之庭','机械层叠与径向展开','从局部结构后退，逐层长出完整的工具花园。'),('B-prism','冰蓝切面','Blender 微距与珐琅材质','真实 Blender 渲染镜头开场，接入切片展开和品牌揭示。'),('C-flow','穿行之间','连续穿梭与数据流','镜头穿过环形通道，展示 Agent → CLI → Linux 的调用路径。'),('D-voxel','世界初生','体素波浪与实录揭晓','方块逐层生长，展开为 Minecraft 客户端主菜单实录。')]
cards=[]
font=ImageFont.truetype('C:/Windows/Fonts/msyh.ttc',23)
sheet=Image.new('RGB',(1440,920),'#eff5fa');draw=ImageDraw.Draw(sheet)
for i,(key,title,tag,desc) in enumerate(studies):
    file=OUT/(key+'.mp4');poster=OUT/(key+'.jpg')
    stamp=5.4 if key!='C-flow' else 4.6
    subprocess.run(['ffmpeg','-v','error','-y','-ss',str(stamp),'-i',str(file),'-frames:v','1','-q:v','2',str(poster)],check=True)
    im=Image.open(poster).convert('RGB');im.thumbnail((712,401));x=i%2*720;y=i//2*460;sheet.paste(im,(x,y+44));draw.text((x+15,y+9),key[0]+' / '+title,fill='#153b5b',font=font)
    cards.append(f'''<section class="study" id="{key}"><div class="study-head"><h2><span>{key[0]}</span>{title}</h2><a download href="{key}.mp4">下载 MP4 ↗</a></div><video controls playsinline preload="metadata" poster="{key}.jpg" src="{key}.mp4"></video><div class="caption"><strong>{tag}</strong><p>{desc}</p></div></section>''')
sheet.save(OUT/'four-directions.jpg',quality=95)
concat=OUT/'render/reel.txt';concat.write_text('\n'.join("file '"+(OUT/(s[0]+'.mp4')).as_posix()+"'" for s in studies),encoding='utf-8')
subprocess.run(['ffmpeg','-v','error','-y','-f','concat','-safe','0','-i',str(concat),'-c','copy','-movflags','+faststart',str(OUT/'four-studies.mp4')],check=True)
page='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Kinakaze / 四种空间叙事</title><style>
*{box-sizing:border-box}body{margin:0;background:#f2f7fb;color:#17394f;font:16px/1.65 "Microsoft YaHei",sans-serif}main{max-width:1560px;margin:auto;padding:52px 48px}header{border-bottom:1px solid #cbdde9;padding-bottom:30px;margin-bottom:36px}.eyebrow{font-size:12px;letter-spacing:.24em;color:#5683a1}h1{font-size:42px;font-weight:500;letter-spacing:-.04em;margin:17px 0 10px}header p{color:#638092;max-width:900px;margin:0}.actions{display:flex;flex-wrap:wrap;gap:12px;margin-top:25px}button,.actions a{font:inherit;padding:10px 20px;border:1px solid #b9d2e3;background:transparent;color:#1d5c8d;text-decoration:none;cursor:pointer}button:hover,.actions a:hover{background:#e2eff8}.grid{display:grid;grid-template-columns:1fr 1fr;gap:38px 28px}.study-head{display:flex;align-items:center;justify-content:space-between;gap:15px;margin-bottom:13px}h2{font-size:23px;font-weight:500;margin:0}h2 span{font-size:15px;letter-spacing:.12em;color:#6796b5;margin-right:15px}a{color:#3d779f}.study-head a{font-size:13px;text-decoration:none}video{width:100%;aspect-ratio:16/9;display:block;background:#e5edf3;box-shadow:0 10px 28px #2449680a}.caption{padding-top:15px}.caption strong{font-size:16px;font-weight:500}.caption p{font-size:14px;color:#688393;margin:5px 0}.note{font-size:12px;color:#718b9b;margin-top:42px;border-top:1px solid #cbdde9;padding-top:20px}.note a{white-space:nowrap}@media(max-width:850px){main{padding:30px 20px}.grid{grid-template-columns:1fr}h1{font-size:32px}}
</style><main><header><div class="eyebrow">KINAKAZE / MOTION STUDIES</div><h1>四种空间叙事。</h1><p>雾白与冰蓝，四种构图、材质和运动节奏。每支约 14 秒，1080p / 60 fps，采用选定的 A 女声。</p><div class="actions"><button id="compare">同步比较 · 静音</button><button id="pause">全部暂停</button><a href="four-studies.mp4">连续观看四支样片 ↗</a></div></header><div class="grid">'''+''.join(cards)+'''</div><div class="note">制作：Blender 建模与微距渲染 / Three.js 空间动画 / Remotion 合成。Blender 微距素材为 30 fps，整体输出为 60 fps。<br>配音为 VITS AI 合成。Music: <a href="https://opentracks.com/bgm/detail/12983">SUMMER TRIANGLE / しゃろう</a>。D 片展示 Minecraft 客户端主菜单，未表示世界内游玩已完成。<br><a href="credits.json">素材署名</a> · <a href="reports/validation.json">导出检查</a></div></main><script>
const videos=[...document.querySelectorAll('video')];let comparing=false;document.getElementById('compare').onclick=()=>{comparing=true;videos.forEach(v=>{v.pause();v.currentTime=0;v.muted=true;v.play().catch(()=>{})})};document.getElementById('pause').onclick=()=>{videos.forEach(v=>v.pause());comparing=false};videos.forEach(v=>v.addEventListener('play',()=>{if(!comparing){videos.filter(x=>x!==v).forEach(x=>x.pause());v.muted=false}}));
</script></html>'''
(OUT/'index.html').write_text(page,encoding='utf-8')
hashes={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in (ROOT/'tools/promo/lab').rglob('*') if p.is_file() and 'node_modules' not in p.parts and p.suffix not in ['.pyc']}
(OUT/'reports/source-hashes.json').write_text(json.dumps(hashes,indent=2),encoding='utf-8')
print('PACKAGED',OUT/'index.html',flush=True)
