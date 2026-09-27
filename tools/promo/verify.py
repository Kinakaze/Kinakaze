"""Inspect the exported movie, its subtitle timing, and its recorded source coverage."""
import json
from pathlib import Path
import subprocess
import wave
import numpy as np
import cv2

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v1'
video=OUT/'kinakaze-promo-v1.mp4'
probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_format','-of','json',str(video)]))
visual=next(s for s in probe['streams'] if s['codec_type']=='video')
audio=next(s for s in probe['streams'] if s['codec_type']=='audio')
timeline=json.loads((OUT/'timeline.json').read_text(encoding='utf-8'))
duration=float(probe['format']['duration'])
assert (visual['width'],visual['height'])==(1920,1080)
assert visual['codec_name']=='h264' and audio['codec_name']=='aac'
assert abs(duration-timeline['duration'])<.15
decode=subprocess.run(['ffmpeg','-hide_banner','-v','error','-i',str(video),'-f','null','-'],capture_output=True)
assert decode.returncode==0 and not decode.stderr,decode.stderr.decode(errors='replace')
with wave.open(str(OUT/'audio/mix.wav'),'rb') as f:
    samples=np.frombuffer(f.readframes(f.getnframes()),dtype='<i2').astype(np.float32)/32768
    peak=float(np.max(np.abs(samples)))
    rms=float(np.sqrt(np.mean(samples**2)))
assert .01<rms<.5 and peak<.999
cap=cv2.VideoCapture(str(video))
sample_times=[2,10,18,22,29,38,42,47,51,58,64,76,80,89,94,101,110,116,123,135,147]
frames=[]
for at in sample_times:
    cap.set(cv2.CAP_PROP_POS_MSEC,at*1000);ok,frame=cap.read();assert ok
    mean=float(frame.mean());assert mean>7,('black frame',at,mean)
    frames.append(dict(seconds=at,mean_brightness=round(mean,2)))
cap.set(cv2.CAP_PROP_POS_MSEC,21800);ok,frame=cap.read();assert ok
cv2.imwrite(str(OUT/'poster.jpg'),frame)
raw={}
for name in ['minecraft','gnome-startup']:
    d=json.loads((OUT/'captures'/f'{name}.json').read_text())
    changes=d['changed_pixel_channel_fraction_per_second']
    raw[name]=dict(seconds=d['video_seconds'],nonidentical_second_pairs=sum(v>0 for v in changes),pairs=len(changes))
for name in ['vi','vim','ssh','java']:
    d=json.loads((OUT/'captures'/f'{name}.json').read_text(encoding='utf8'))
    raw[name]=dict(seconds=d['duration'],terminal_events=len(d['events']))
java=json.loads((OUT/'captures/java.json').read_text(encoding='utf8'))
assert 'JAVA_SPAWN_OK' in ''.join(e[2] for e in java['events'])
ssh=json.loads((OUT/'reports/ssh-result.json').read_text())
assert ssh['returncode']==0 and 'PIPELINE COMPLETE' in ssh['stdout']
report=dict(status='passed',seconds=duration,width=1920,height=1080,fps=visual['avg_frame_rate'],
            bytes=video.stat().st_size,video_codec=visual['codec_name'],audio_codec=audio['codec_name'],
            audio_peak=peak,audio_rms=rms,full_decode_errors=0,frames=frames,sources=raw,
            limitations=['GNOME startup display only; desktop interactions were not verified.',
                         'Minecraft main menu and Continue response only; in-world gameplay not verified.',
                         'Terminal recordings are rendered replays of real timestamped ConPTY output.'])
(OUT/'reports/video-validation.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
