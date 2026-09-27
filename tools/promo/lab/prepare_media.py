"""Build four original sound edits, retaining selected VITS A and credited music."""
import json,shutil,subprocess
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from scipy.signal import butter,sosfilt,resample_poly

ROOT=Path(__file__).resolve().parents[3]
OUT=ROOT/'artifacts/promo-lab';PUB=OUT/'public'
(PUB/'audio').mkdir(parents=True,exist_ok=True)
shutil.copy2('C:/Windows/Fonts/NotoSansSC-VF.ttf',PUB/'NotoSansSC.ttf')
shutil.copy2('C:/Windows/Fonts/calibril.ttf',PUB/'calibril.ttf')
shutil.copy2(ROOT/'artifacts/promo-v3/assets/minecraft-clean.mp4',PUB/'minecraft.mp4')
sr=48000;duration=824/60;n=round(duration*sr)
source=ROOT/'artifacts/promo-v3/assets/summer-triangle.mp3'
music=np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-i',str(source),'-f','f32le','-ac','2','-ar',str(sr),'-']),dtype='<f4').reshape(-1,2)
entries=[]
for idx,(kind,offset,start) in enumerate([('bloom',48*60/175,1.05),('prism',16*60/175,3.85),('flow',128*60/175,.92),('voxel',224*60/175,.85)]):
    r,voice=wavfile.read(OUT/'voices'/('abcd'[idx]+'.wav'));voice=resample_poly(voice.astype(np.float64)/32768,sr,r)
    voice*=.108/max(.001,np.sqrt(np.mean(voice**2)))
    t=np.arange(n)/sr
    a=start;b=start+len(voice)/sr
    duck=1-.66*np.clip((t-a+.3)/.3,0,1)*np.clip((b+.45-t)/.45,0,1)
    fade=np.minimum(np.clip(t/.35,0,1),np.clip((duration-t)/.7,0,1))
    mix=music[round(offset*sr):round(offset*sr)+n].astype(float)*.40*duck[:,None]*fade[:,None]
    at=round(start*sr);mix[at:at+len(voice)]+=voice[:,None]*.82
    rng=np.random.default_rng(91+idx)
    events=[.18,2.742857,5.485714,8.228571,10.971429]
    for k,when in enumerate(events):
        length=.38 if k else .58;nt=round(length*sr);tt=np.arange(nt)/sr
        noise=sosfilt(butter(2,[850,5900],btype='bandpass',fs=sr,output='sos'),rng.normal(size=nt))
        env=np.sin(np.pi*tt/length)**2
        sweep=noise*env*.009+np.sin(2*np.pi*(520*tt+700*tt**2))*np.exp(-tt*19)*.011
        pos=round(when*sr);pan=(-.35+k*.17)
        mix[pos:pos+nt]+=np.stack([sweep*(1-pan),sweep*(1+pan)],axis=1)
    temp=OUT/'voices'/(kind+'-premix.wav');wavfile.write(temp,sr,mix.astype(np.float32))
    target=PUB/'audio'/(kind+'.wav')
    p=subprocess.run(['ffmpeg','-v','info','-y','-i',str(temp),'-af','loudnorm=I=-16:TP=-1.7:LRA=8:print_format=json','-ar','48000','-c:a','pcm_s16le',str(target)],capture_output=True,text=True,check=True)
    loudness=json.JSONDecoder().raw_decode(p.stderr[p.stderr.rfind('{'):])[0]
    entries.append({'kind':kind,'voice':'abcd'[idx],'voice_start':start,'voice_seconds':len(voice)/sr,'music_start':offset,'duration':duration,'loudness':loudness})
    print(kind,round(len(voice)/sr,2),'seconds speech',flush=True)
(OUT/'reports/audio.json').write_text(json.dumps(entries,indent=2),encoding='utf-8')
(OUT/'credits.json').write_text(json.dumps({'music':{'title':'SUMMER TRIANGLE','artist':'しゃろう','source':'https://opentracks.com/bgm/detail/12983'},'voice':{'engine':'VITS','speaker':1,'speed':1.15,'selection':'A','source':'https://huggingface.co/guetLzy/VITS-fast-fine-tuning'},'geometry':'Original geometry authored procedurally in Blender 5.2.1; exported GLB with editable .blend scenes','animation':'Three.js 0.186.1 / React Three Fiber; deterministic frame animation','composition':'Remotion 4.0.529','sample_length':duration},ensure_ascii=False,indent=2),encoding='utf-8')
