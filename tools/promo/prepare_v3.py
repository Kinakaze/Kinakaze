"""Prepare verified captures, timed terminal states and the keynote sound edit."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import numpy as np
import render_v2 as r

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v3'
manifest=json.loads((OUT/'audio/manifest.json').read_text(encoding='utf-8'))
tl=[]
for s in manifest['segments']:
    assert s['audio_seconds']+.55<s['duration'],(s['id'],s['audio_seconds'],s['duration'])
    tl.append(dict(s,voice_start=s['start']+.4))
r.OUT=OUT
for target,original,offset,seconds in [('gnome','gnome-startup',4.5,20),('gnome-settings','gnome',1,15),('minecraft','minecraft',0,26)]:
    target_file=OUT/'assets'/f'{target}-clean.mp4'
    if not target_file.exists():
        subprocess.run(['ffmpeg','-v','error','-y','-ss',str(offset),'-i',str(ROOT/'artifacts/promo-v1/captures'/f'{original}.mp4'),
                        '-t',str(seconds),'-c:v','h264_nvenc','-preset','p4','-cq','17','-an','-movflags','+faststart',str(target_file)],check=True)
shutil.copyfile(ROOT/'artifacts/promo-v2/assets/summer-triangle.mp3',OUT/'assets/summer-triangle.mp3')
shutil.copyfile(ROOT/'artifacts/promo-v2/reports/music-source.json',OUT/'reports/music-source.json')
r.audio_mix(tl,127.9)
cues=r.subtitles(tl)
# Original captions are exported under the final version name.
(OUT/'kinakaze-promo-v2.srt').replace(OUT/'kinakaze-promo-v3.srt')
data=dict(title=manifest['title'],duration=127.9,segments=tl,cues=cues)
states={}
for name in ('vi','vim','ssh','java'):
    term=r.Terminal(name);samples=[]
    for p in np.linspace(0,.999,241):
        term.image(float(p))
        samples.append(dict(lines=list(term.screen.display),x=term.screen.cursor.x,y=term.screen.cursor.y,hidden=term.screen.cursor.hidden))
    states[name]=samples
data['terminals']=states
data['agent']=json.loads((OUT/'reports/agent-demo.json').read_text(encoding='utf-8'))
(OUT/'assets/film.json').write_text(json.dumps(data,ensure_ascii=False),encoding='utf-8')
(OUT/'timeline.json').write_text(json.dumps({k:v for k,v in data.items() if k in ('title','duration','segments','cues')},ensure_ascii=False,indent=2),encoding='utf-8')
print('Prepared 127.9 s with real terminal states, verified CLI roundtrip, chosen voice A and music.')
