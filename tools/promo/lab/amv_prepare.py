"""Build a measured beat timeline, source evidence and the new AMV mix."""
from pathlib import Path
import json,shutil,subprocess
import numpy as np
from scipy.io import wavfile
from scipy.signal import resample_poly,butter,sosfilt

ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-amv';PUB=OUT/'public'
SETTINGS=json.loads((Path(__file__).parent/'amv-settings.json').read_text(encoding='utf-8'))
FPS=SETTINGS['fps'];BPM=125;BEAT=60/BPM;BEATS=112;DURATION=BEATS*BEAT;FRAMES=round(DURATION*FPS)
assert FRAMES==SETTINGS['frames']
analysis=json.loads((OUT/'reports/beat-analysis.json').read_text())
assert abs(analysis['bpm']-BPM)<.05
# The fitted flux maximum follows the first audible attack by about 20 ms.
# Starting 80 beats into the song gives a four-bar lift before the main drop.
phase=analysis['phase_seconds']-.020;offset=80*BEAT+phase
shots=[
 (0,4,'eyes','角色近景'),(4,8,'hello','你好 Windows'),(8,12,'invitation','少女打开界面'),
 (12,16,'pickup','四拍加速切镜'),(16,24,'launch','重拍品牌登场'),
 (24,32,'vi','vi 输入与编辑'),(32,40,'vim','Vim 保存与读回'),(40,48,'ssh','SSH / sshd 实录'),
 (48,56,'desktop','GNOME 实录'),(56,72,'cli','CLI 管道、重定向与文件读回'),
 (72,80,'java','Linux Java 实际输出'),(80,92,'minecraft','Minecraft 主菜单实录'),
 (92,104,'reprise','角色近景 · 柔和收束'),(104,112,'end','项目名与开源邀请')]
timeline={'title':'Kinakaze · 你好，Windows。','fps':FPS,'bpm':BPM,'beat':BEAT,'frames':FRAMES,'duration':FRAMES/FPS,'music_offset':offset,'drop_frame':round(16*BEAT*FPS),'beats':[round(i*BEAT*FPS) for i in range(BEATS+1)],'shots':[{'in':a,'out':b,'id':k,'label':n,'from':round(a*BEAT*FPS),'to':round(b*BEAT*FPS)} for a,b,k,n in shots]}
film=json.loads((ROOT/'artifacts/promo-v3/assets/film.json').read_text(encoding='utf-8'))
cli=json.loads((OUT/'reports/cli-demo-v3.json').read_text(encoding='utf-8'))
(PUB/'evidence.json').write_text(json.dumps({'terminals':film['terminals'],'cli':cli},ensure_ascii=False),encoding='utf-8')
for src,dst in [('gnome-settings-clean.mp4','gnome.mp4'),('minecraft-clean.mp4','minecraft.mp4')]:shutil.copy2(ROOT/'artifacts/promo-v3/assets'/src,PUB/dst)
shutil.copy2('C:/Windows/Fonts/NotoSansSC-VF.ttf',PUB/'NotoSansSC.ttf')
shutil.copy2('C:/Windows/Fonts/consola.ttf',PUB/'consola.ttf')

sr=48000;n=round(timeline['duration']*sr);t=np.arange(n)/sr
music=np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-ss',str(offset),'-i',str(PUB/'audio/honey-lemon.mp3'),'-t',str(timeline['duration']),'-ar',str(sr),'-ac','2','-f','f32le','-']),np.float32).reshape(-1,2)
assert len(music)>=n-48
if len(music)<n:music=np.pad(music,((0,n-len(music)),(0,0)))
mix=music[:n].astype(float)*.72
voices=[('launch',8.5*BEAT),('cli',62*BEAT),('end',106.1*BEAT)]
voice_records=[]
for key,start in voices:
    r,v=wavfile.read(OUT/'voices'/(key+'.wav'));v=v.astype(float)/32768
    if v.ndim>1:v=v.mean(axis=1)
    v=resample_poly(v,sr,r);v*=.13/max(.001,np.sqrt(np.mean(v*v)))
    end=start+len(v)/sr;duck=1-.61*np.clip((t-start+.10)/.10,0,1)*np.clip((end+.16-t)/.16,0,1)
    mix*=duck[:,None];i=round(start*sr);mix[i:i+len(v)]+=v[:,None]*.9
    voice_records.append({'id':key,'start':start,'end':end})
# Subtle sweet transition accents. The track supplies the main drums.
rng=np.random.default_rng(125)
for beat in [s[0] for s in shots if 0<s[0]<92]:
    at=round(beat*BEAT*sr);length=round(.27*sr);tt=np.arange(length)/sr
    noise=sosfilt(butter(2,[1600,7000],btype='bandpass',fs=sr,output='sos'),rng.normal(size=length))
    accent=noise*np.sin(np.pi*tt/.27)**3*.006
    accent+=np.sin(2*np.pi*(1174.66*tt+450*tt*tt))*np.exp(-tt*32)*.014
    mix[at:at+length]+=accent[:,None]
starts={s['id']:s['from']/FPS for s in timeline['shots']}
feedback=[('vi',3.65),('vim',1.95),('cli',.98),('cli',3.42),('cli',4.46)]
for key,local in feedback:
    at=round((starts[key]+local)*sr);length=round(.055*sr);tt=np.arange(length)/sr
    tap=(np.sin(2*np.pi*740*tt)*.017+np.sin(2*np.pi*1110*tt)*.005)*np.exp(-tt*105)
    tap*=np.minimum(tt/.002,1)
    mix[at:at+length]+=tap[:,None]
timeline['ui_feedback']=[{'scene':key,'at':starts[key]+local,'kind':'quiet command confirmation'} for key,local in feedback]
fade=np.minimum(np.clip(t/.08,0,1),np.clip((timeline['duration']-t)/.65,0,1));mix*=fade[:,None]
premix=OUT/'render/premix.wav';wavfile.write(premix,sr,mix.astype(np.float32))
subprocess.run(['ffmpeg','-v','error','-y','-i',str(premix),'-af','loudnorm=I=-14:TP=-1.2:LRA=7','-ar','48000','-c:a','pcm_s16le',str(PUB/'audio/mix.wav')],check=True)
assert next(v['end'] for v in voice_records if v['id']=='launch')<16*BEAT
timeline['voices']=voice_records
timeline['render']={'width':SETTINGS['width'],'height':SETTINGS['height'],'fps':SETTINGS['fps'],'filename':SETTINGS['filename']}
(PUB/'timeline.json').write_text(json.dumps(timeline,ensure_ascii=False,indent=2),encoding='utf-8')
(OUT/'timeline.json').write_text(json.dumps(timeline,ensure_ascii=False,indent=2),encoding='utf-8')
(OUT/'credits.json').write_text(json.dumps({'music':json.loads((OUT/'reports/music-source.json').read_text(encoding='utf-8')),'character':'Original silver-haired heroine; built-in imagegen, redesigned after user feedback; no existing anime footage','voice':'VITS, previously selected voice A, speaker 1 / speed 1.15','terminal':'Recorded terminal states, with a larger cropped viewport and edited timing; vi/Vim source sessions used SSH TTY transport. CLI pipeline commands are directly executed through worker.','cli':cli['scope'],'minecraft':'Linux client main-menu capture, not world gameplay'},ensure_ascii=False,indent=2),encoding='utf-8')
print('AMV',FRAMES,'frames',timeline['duration'],'seconds. Music start',offset,'drop',16*BEAT)
