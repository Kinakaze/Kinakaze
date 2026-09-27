"""Validate the actual media export and the claims supported by recorded evidence."""
import json
from pathlib import Path
import re
import subprocess
import wave
import cv2
import numpy as np

ROOT=Path(__file__).resolve().parents[2];OUT=ROOT/'artifacts/promo-v3';video=OUT/'kinakaze-promo-v3.mp4'
probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_format','-of','json',str(video)]))
vs=next(s for s in probe['streams'] if s['codec_type']=='video');au=next(s for s in probe['streams'] if s['codec_type']=='audio')
assert (vs['width'],vs['height'])==(1920,1080) and vs['avg_frame_rate']=='60/1'
assert abs(float(probe['format']['duration'])-127.9)<.15
decode=subprocess.run(['ffmpeg','-v','error','-i',str(video),'-f','null','-'],capture_output=True)
assert decode.returncode==0 and not decode.stderr,decode.stderr.decode(errors='replace')
browser=json.loads((OUT/'reports/browser-render.json').read_text());assert not browser['errors'];assert browser['frames']==7674
agent=json.loads((OUT/'reports/agent-demo.json').read_text());assert agent['status']=='passed' and agent['exit_code']==0
assert agent['stdout'].replace('\r','')==agent['host_readback']
tl=json.loads((OUT/'timeline.json').read_text(encoding='utf8'))
for s in tl['segments']:assert s['voice_start']+s['audio_seconds']<s['start']+s['duration']
with wave.open(str(OUT/'audio/mix.wav'),'rb') as f:
    audio=np.frombuffer(f.readframes(f.getnframes()),dtype='<i2').astype(np.float32)/32768
peak=float(np.max(np.abs(audio)));rms=float(np.sqrt(np.mean(audio**2)));assert .03<rms<.5 and peak<.999
cap=cv2.VideoCapture(str(video));frames=[];samples=[3,10,20,25.5,31.8,40,46,52,58,64.5,72,75,83,92.8,98.5,102,107,115,124]
for i,t in enumerate(samples):
    cap.set(cv2.CAP_PROP_POS_MSEC,t*1000);ok,a=cap.read();assert ok and a.mean()>10
    cv2.imwrite(str(OUT/'render'/f'export-{i:02}.jpg'),a)
    frames.append(dict(at=t,brightness=round(float(a.mean()),2)))
motion=[]
for t in [2,18,30,37,45,57,63,70,82,92,100,106,113,123]:
    cap.set(cv2.CAP_PROP_POS_MSEC,t*1000);ok,a=cap.read();assert ok
    cap.set(cv2.CAP_PROP_POS_MSEC,(t+.5)*1000);ok,b=cap.read();assert ok
    change=float(np.mean(np.abs(a.astype(np.float32)-b.astype(np.float32))));assert change>.005,(t,change)
    motion.append(dict(at=t,mean_pixel_change=round(change,3)))
loud=subprocess.run(['ffmpeg','-hide_banner','-i',str(video),'-af','loudnorm=I=-16:TP=-1.5:LRA=9:print_format=json','-f','null','-'],capture_output=True,text=True)
m=re.search(r'\{\s*"input_i".*?\}',loud.stderr,re.S);levels=json.loads(m[0]) if m else {}
report=dict(status='passed',duration=float(probe['format']['duration']),width=1920,height=1080,fps='60/1',frames=browser['frames'],
            bytes=video.stat().st_size,video_codec=vs['codec_name'],audio_codec=au['codec_name'],full_decode_errors=0,
            browser_errors=browser['errors'],gpu=browser['gpu'],audio_peak=peak,audio_rms=rms,loudness=levels,
            agent_tool_call=agent['status'],sample_frames=frames,motion_samples=motion,
            music=json.loads((OUT/'reports/music-source.json').read_text(encoding='utf8')),
            scope=['Actual host CLI tool call with output, exit code and host file readback.',
                   'Files in the prepared guest root; no claim of arbitrary host mounts or bundled MCP integration.',
                   'GNOME startup views only, with editorial 3D camera movement.',
                   'Minecraft main menu/Continue only; in-world gameplay not verified.',
                   'Motion graphics at 60 fps; software capture source at 24 fps.'])
(OUT/'reports/video-validation.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({k:v for k,v in report.items() if k in ('status','duration','fps','frames','bytes','full_decode_errors','audio_peak','loudness','agent_tool_call')},ensure_ascii=False))
