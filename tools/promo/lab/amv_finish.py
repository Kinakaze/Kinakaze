"""Validate the actual export and package a small local screening page."""
from pathlib import Path
import subprocess,json,hashlib
import numpy as np

ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-amv'
settings=json.loads((Path(__file__).parent/'amv-settings.json').read_text(encoding='utf-8'))
movie=OUT/settings['filename'];fps=settings['fps'];frames=settings['frames']
render_report=json.loads((OUT/f"reports/render-full-{settings['reportSuffix']}.json").read_text(encoding='utf-8'))
assert not render_report['errors'],render_report['errors']
assert render_report['drawingBuffers']==[f"AMV_DRAWING_BUFFER {settings['width']}x{settings['height']}"],render_report
timeline=json.loads((OUT/'timeline.json').read_text(encoding='utf-8'))
# Remotion's intermediate audio added two 48 kHz AAC packets of leading
# delay. Mux directly from the approved PCM mix, retaining every video frame.
synced=OUT/'render/audio-synced.mp4'
subprocess.run(['ffmpeg','-v','error','-y','-i',str(movie),'-i',str(OUT/'public/audio/mix.wav'),'-map','0:v:0','-map','1:a:0','-c:v','copy','-c:a','aac','-b:a','320k','-t',str(timeline['duration']),'-movflags','+faststart',str(synced)],check=True)
synced.replace(movie)
metadata=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_format','-show_streams','-of','json',str(movie)]))
v=next(s for s in metadata['streams'] if s['codec_type']=='video')
a=next(s for s in metadata['streams'] if s['codec_type']=='audio')
assert (v['width'],v['height'],v['r_frame_rate'],int(v['nb_frames']))==(settings['width'],settings['height'],f'{fps}/1',frames)
assert abs(float(v['duration'])-timeline['duration'])<.02
decoded=subprocess.run(['ffmpeg','-v','error','-i',str(movie),'-f','null','-'],capture_output=True,text=True)
assert decoded.returncode==0 and not decoded.stderr.strip(),decoded.stderr
levels=subprocess.run(['ffmpeg','-v','info','-i',str(movie),'-af','loudnorm=I=-14:TP=-1.2:LRA=7:print_format=json','-vn','-f','null','-'],capture_output=True,text=True,check=True).stderr
loud=json.JSONDecoder().raw_decode(levels[levels.rfind('{'):])[0]
assert -16<float(loud['input_i'])<-12
assert float(loud['input_tp'])<-.1

def mono(p):
    return np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-i',str(p),'-t','6','-ar','12000','-ac','1','-f','f32le','-']),np.float32).astype(float)
x=mono(movie);y=mono(OUT/'public/audio/mix.wav');length=min(len(x),len(y));x=x[:length];y=y[:length]
x-=x.mean();y-=y.mean();size=2**int(np.ceil(np.log2(length*2)))
corr=np.fft.irfft(np.fft.rfft(x,size)*np.conj(np.fft.rfft(y,size)),size)
lags=np.arange(-1800,1801);best=int(lags[np.argmax(corr[lags%size])]);lag_seconds=best/12000
assert abs(lag_seconds)<1/fps,(best,lag_seconds)
beat_errors=[abs(s['from']/fps-s['in']*timeline['beat']) for s in timeline['shots']]
assert max(beat_errors)<1/(2*fps)+.0001
shots={s['id']:s for s in timeline['shots']}
motion=[]
for at in [.55,2.45,4.45,shots['vi']['from']/fps+.9,shots['cli']['from']/fps+.4,shots['minecraft']['from']/fps+1.2,shots['reprise']['from']/fps+.4,shots['end']['from']/fps+1.1]:
    raw=subprocess.check_output(['ffmpeg','-v','error','-ss',str(at),'-i',str(movie),'-vf','fps=4,scale=240:135','-frames:v','2','-pix_fmt','rgb24','-f','rawvideo','-'])
    pixels=np.frombuffer(raw,np.uint8).reshape(2,135,240,3).astype(float)
    delta=float(np.abs(pixels[1]-pixels[0]).mean());assert delta>.005,(at,delta)
    motion.append({'at':at,'mean_pixel_delta':delta})
# A desktop recording can pause between UI events. With the artificial zoom
# removed, check fidelity to the source instead of requiring continuous motion.
def frame(p,at):
    raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(p),'-ss',str(at),'-vf','scale=240:135','-frames:v','1','-pix_fmt','rgb24','-f','rawvideo','-'])
    return np.frombuffer(raw,np.uint8).reshape(135,240,3).astype(float)
desktop_export=frame(movie,shots['desktop']['from']/fps+1.8);desktop_source=frame(OUT/'public/gnome.mp4',4.8)
desktop_error=float(np.abs(desktop_export[16:100,30:230]-desktop_source[16:100,30:230]).mean())
assert desktop_error<5,desktop_error
report={'status':'passed','file':movie.name,'frames':frames,'video_seconds':float(v['duration']),'container_seconds':float(metadata['format']['duration']),'size':[v['width'],v['height']],'fps':fps,'drawing_buffers':render_report['drawingBuffers'],'capture_format':render_report['captureFormat'],'decode_errors':0,'loudness':loud,'measured_export_audio_lag_seconds':lag_seconds,'largest_cut_rounding_error_seconds':max(beat_errors),'motion':motion,'desktop_source_mean_absolute_error':desktop_error,'sha256':hashlib.sha256(movie.read_bytes()).hexdigest()}
latest=OUT/'reports/validation.json'
if latest.exists():
    previous=json.loads(latest.read_text(encoding='utf-8'))
    if previous.get('file')=='kinakaze-amv-v1.mp4':
        (OUT/'reports/validation-v1.json').write_text(json.dumps(previous,indent=2),encoding='utf-8')
for target in [latest,OUT/f"reports/validation-{settings['reportSuffix']}.json"]:
    target.write_text(json.dumps(report,indent=2),encoding='utf-8')
subprocess.run(['ffmpeg','-v','error','-y','-ss','8.4','-i',str(movie),'-frames:v','1','-q:v','2',str(OUT/'poster.jpg')],check=True)
chapters=[(shots[key]['from']/fps,label) for key,label in [('eyes','开场'),('launch','登场'),('vi','vi / Vim'),('ssh','SSH'),('desktop','桌面'),('cli','CLI 文件操作'),('java','Java'),('minecraft','Minecraft'),('end','发布')]]
buttons=''.join(f'<button data-time="{t:g}">{label}</button>' for t,label in chapters)
page='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Kinakaze · 你好，Windows。</title><style>
@font-face{font-family:Noto;src:url(public/NotoSansSC.ttf);font-weight:100 900}*{box-sizing:border-box}body{margin:0;background:#eff7fd;color:#254e78;font:16px/1.7 Noto,sans-serif}main{max-width:1400px;margin:auto;padding:40px}.eyebrow{font-size:12px;letter-spacing:.24em;color:#6b91bb}h1{font-weight:550;letter-spacing:-.045em;font-size:40px;margin:8px 0 13px}header{display:flex;align-items:flex-end;justify-content:space-between;margin-bottom:24px;gap:25px}header p{margin:0;color:#6387aa;font-size:14px}video{display:block;width:100%;aspect-ratio:16/9;background:#d7eafb;box-shadow:0 15px 60px #6994c418}nav{display:flex;flex-wrap:wrap;gap:10px;margin:22px 0}button,a.download{font:inherit;color:#356da4;border:1px solid #b6d3ee;padding:8px 18px;background:transparent;cursor:pointer;text-decoration:none}button:hover,a.download:hover{background:#e1effc}footer{margin-top:27px;font-size:12px;line-height:2;color:#7b9bb7}a{color:#4a86bd}@media(max-width:700px){main{padding:22px 14px}header{display:block}h1{font-size:31px}.download{display:inline-block;margin-top:15px}}
</style><main><header><div><div class="eyebrow">KINAKAZE / AMV EDIT 04 / CLI</div><h1>你好，Windows。</h1><p>原创银白发少女 · 音乐卡点 · 实际功能演示　/　{{DURATION}} 秒 · {{RENDER_SIZE}}</p></div><a class="download" href="{{MOVIE}}" download>下载样片 ↗</a></header><video id="film" controls playsinline preload="metadata" src="{{MOVIE}}" poster="poster.jpg"></video><nav>'''+buttons+'''</nav><footer>角色采用动态立绘与镜头剪辑；原画由 imagegen 生成。配音沿用选定的 A 女声，VITS 合成。<br>Music: <a href="https://opentracks.com/bgm/detail/14621">しゅわしゅわハニーレモン350ml / しゃろう</a> · <a href="credits.json">素材记录</a> · <a href="reports/validation.json">导出检查</a><br>2K 合成输出；桌面与游戏录屏源为 1080p。Minecraft 展示客户端主菜单；CLI 展示真实命令、输出与文件读回；输入速度按音乐节奏剪辑。</footer></main><script>const v=document.getElementById('film');document.querySelectorAll('button[data-time]').forEach(b=>b.onclick=()=>{v.currentTime=Number(b.dataset.time);v.play().catch(e=>{if(e.name!=='AbortError')console.error(e)})});</script></html>'''
page=page.replace('{{RENDER_SIZE}}',f"{settings['width']}×{settings['height']} · {fps} fps")
page=page.replace('{{MOVIE}}',movie.name)
page=page.replace('{{DURATION}}',f"{timeline['duration']:.2f}")
(OUT/'index.html').write_text(page,encoding='utf-8')
print(json.dumps({'status':'passed','duration':v['duration'],'audio_lag_seconds':lag_seconds,'largest_cut_rounding_error_ms':max(beat_errors)*1000,'loudness':loud['input_i'],'file':str(movie)},indent=2))
