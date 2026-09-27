"""Check complete decoding, audio, changing images and promised sample dimensions."""
import subprocess,json
from pathlib import Path
import numpy as np
ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-lab'
reports=[]
for key in ['A-bloom','B-prism','C-flow','D-voxel']:
    file=OUT/(key+'.mp4')
    j=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_format','-show_streams','-of','json',str(file)]))
    video=next(s for s in j['streams'] if s['codec_type']=='video');audio=next(s for s in j['streams'] if s['codec_type']=='audio')
    assert(video['width'],video['height'],video['r_frame_rate'])==(1920,1080,'60/1')
    assert int(video['nb_frames'])==824
    assert abs(float(j['format']['duration'])-824/60)<.1
    dec=subprocess.run(['ffmpeg','-v','error','-i',str(file),'-f','null','-'],capture_output=True,text=True)
    assert dec.returncode==0 and not dec.stderr.strip(),dec.stderr
    levels=subprocess.run(['ffmpeg','-v','info','-i',str(file),'-af','loudnorm=I=-16:TP=-1.7:LRA=8:print_format=json','-vn','-f','null','-'],capture_output=True,text=True,check=True).stderr
    loud=json.JSONDecoder().raw_decode(levels[levels.rfind('{'):])[0]
    assert -18<float(loud['input_i'])<-14
    assert float(loud['input_tp'])<-.3
    motion=[]
    for t in [1.3,4.6,8.6,11.4]:
        raw=subprocess.check_output(['ffmpeg','-v','error','-ss',str(t),'-i',str(file),'-vf','fps=4,scale=192:108','-frames:v','2','-pix_fmt','rgb24','-f','rawvideo','-'])
        a=np.frombuffer(raw,np.uint8).reshape(2,108,192,3).astype(float)
        delta=float(np.mean(np.abs(a[1]-a[0])));assert delta>.06,(key,t,delta);motion.append({'at':t,'mean_pixel_delta':delta})
    reports.append({'file':file.name,'duration':float(j['format']['duration']),'frames':int(video['nb_frames']),'fps':60,'size':[1920,1080],'bytes':file.stat().st_size,'decode_errors':0,'audio_codec':audio['codec_name'],'loudness':loud,'motion':motion})
    print('PASS',key,flush=True)
(OUT/'reports/validation.json').write_text(json.dumps({'status':'passed','samples':reports},indent=2),encoding='utf-8')
