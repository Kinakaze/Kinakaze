"""Fetch the verified official track and measure its pulse before editing."""
from pathlib import Path
import hashlib,json,re,subprocess
import numpy as np
import requests

ROOT=Path(__file__).resolve().parents[3]
OUT=ROOT/'artifacts/promo-amv'
for name in ['public/art','public/audio','voices','reports','stills','render']:
    (OUT/name).mkdir(parents=True,exist_ok=True)

url='https://opentracks.com/bgm/detail/14621/download'
s=requests.Session();s.mount('https://',requests.adapters.HTTPAdapter(max_retries=3));page=s.get(url,timeout=45);page.raise_for_status();page.encoding='utf-8'
assert 'しゅわしゅわハニーレモン350ml' in page.text
token=re.search(r'name="csrfmiddlewaretoken" value="([^"]+)"',page.text)[1]
audio=s.post(url,data={'csrfmiddlewaretoken':token,'track':'1'},headers={'Referer':url},timeout=60)
audio.raise_for_status();assert len(audio.content)>500000 and 'html' not in audio.headers.get('Content-Type','')
src=OUT/'public/audio/honey-lemon.mp3';src.write_bytes(audio.content)
report={'title':'しゅわしゅわハニーレモン350ml','composer':'しゃろう','source':'https://opentracks.com/bgm/detail/14621','license':'https://opentracks.com/help/articles/license/','bytes':len(audio.content),'sha256':hashlib.sha256(audio.content).hexdigest()}
(OUT/'reports/music-source.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
sr=12000;hop=120;win=1024
y=np.frombuffer(subprocess.check_output(['ffmpeg','-v','error','-i',str(src),'-ac','1','-ar',str(sr),'-f','f32le','-']),np.float32)
frames=np.lib.stride_tricks.sliding_window_view(y,win)[::hop]
spec=np.abs(np.fft.rfft(frames*np.hanning(win),axis=1))
freq=np.fft.rfftfreq(win,1/sr)
low=np.log1p(spec[:,(freq>40)&(freq<190)]).mean(axis=1)
high=np.log1p(spec[:,(freq>900)&(freq<6500)]).mean(axis=1)
onset=np.maximum(0,np.diff(low,prepend=low[0]))+.65*np.maximum(0,np.diff(high,prepend=high[0]))
# Center the analysis windows in time, then fit the pulse over the full song.
times=(np.arange(len(onset))*hop+win/2)/sr
candidates=[]
for bpm in np.arange(122,128.01,.025):
    step=60/bpm
    for phase in np.arange(0,step,.005):
        grid=np.arange(phase+step*8,len(y)/sr-5,step)
        score=np.interp(grid,times,onset).mean()
        candidates.append((float(score),float(bpm),float(phase)))
best=max(candidates)
bpm=round(best[1],3);phase=round(best[2],4)
energy=[{'start':round(t,2),'rms':round(float(np.sqrt(np.mean(y[int(t*sr):int((t+1)*sr)]**2))),5)} for t in np.arange(0,len(y)/sr-1,1)]
result={'bpm':bpm,'phase_seconds':phase,'score':best[0],'source_seconds':len(y)/sr,'top_fits':sorted(candidates,reverse=True)[:8],'energy':energy,'method':'Positive spectral flux, low and high frequency bands; whole-track pulse-grid fit, 10 ms analysis hop.'}
(OUT/'reports/beat-analysis.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
np.savez_compressed(OUT/'reports/onsets.npz',times=times,onset=onset)
print(json.dumps({k:result[k] for k in ['bpm','phase_seconds','score','source_seconds','top_fits']},indent=2))
