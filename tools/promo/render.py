"""Render the first cut from real recordings, VITS speech, and original motion graphics."""
import argparse
import bisect
import copy
import functools
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import wave

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v1'
sys.path.insert(0,str(OUT/'python'))
import pyte

W,H,FPS=1920,1080,24
MINT=(134,246,221)
PINK=(255,158,202)
WHITE=(239,244,255)
MUTED=(157,174,199)
BG=(11,16,36)


@functools.lru_cache(maxsize=32)
def font(size,bold=False,mono=False):
    name='consola.ttf' if mono else ('msyhbd.ttc' if bold else 'msyh.ttc')
    return ImageFont.truetype('C:/Windows/Fonts/'+name,size)


def text(draw,xy,value,size=40,fill=WHITE,bold=False,anchor=None,mono=False):
    draw.text(xy,value,font=font(size,bold,mono),fill=fill,anchor=anchor,stroke_width=0)


def pill(draw,xy,value,accent=MINT):
    x,y=xy
    width=int(draw.textlength(value,font=font(23,True)))+34
    draw.rounded_rectangle((x,y,x+width,y+42),radius=12,fill=(25,38,58),outline=accent,width=1)
    text(draw,(x+17,y+6),value,23,accent,True)


def ease(v):
    v=max(0,min(v,1));return 1-(1-v)**3


def read_wav(path):
    with wave.open(str(path),'rb') as f:
        rate=f.getframerate(); channels=f.getnchannels()
        y=np.frombuffer(f.readframes(f.getnframes()),dtype='<i2').astype(np.float32)/32768
    if channels>1:y=y.reshape(-1,channels).mean(1)
    return rate,y


def write_wav(path,rate,audio):
    with wave.open(str(path),'wb') as f:
        f.setnchannels(1 if audio.ndim==1 else audio.shape[1]);f.setsampwidth(2);f.setframerate(rate)
        f.writeframes((np.clip(audio,-1,1)*32767).astype('<i2').tobytes())


def synth_music(seconds,rate=44100):
    """Original light electronic instrumental, no borrowed recording or melody."""
    n=math.ceil(seconds*rate);mix=np.zeros((n,2),np.float32)
    rng=np.random.default_rng(20260926);beat=60/132
    chords=[[64,68,71,76],[59,63,66,71],[61,64,68,73],[57,61,64,69]]
    def add(when,sound,gain=.2,pan=0):
        start=int(when*rate);end=min(n,start+len(sound))
        if start>=n:return
        size=end-start
        mix[start:end,0]+=sound[:size]*gain*math.sqrt((1-pan)/2)
        mix[start:end,1]+=sound[:size]*gain*math.sqrt((1+pan)/2)
    def note(midi,duration,pluck=True):
        t=np.arange(int(duration*rate))/rate;f=440*2**((midi-69)/12)
        sound=np.sin(2*np.pi*f*t)+.28*np.sin(4*np.pi*f*t)+.1*np.sin(6*np.pi*f*t)
        env=(1-np.exp(-t*65))*np.exp(-t*(6 if pluck else .75))
        env*=np.minimum(1,(duration-t)/.08)
        return (sound*env).astype(np.float32)
    for bar in range(math.ceil(seconds/(beat*4))):
        chord=chords[bar%4];at=bar*beat*4
        for midi in chord:add(at,note(midi,beat*4,False),.06)
        for k,index in enumerate([0,2,1,3,2,1,3,2]):
            add(at+k*beat/2,note(chord[index]+12,beat*.8),.09,(-.32 if k%2 else .32))
        for k in range(4):
            tt=np.arange(int(.18*rate))/rate
            kick=np.sin(2*np.pi*(43*tt+38*.05*(1-np.exp(-tt/.05))))*np.exp(-tt*24)
            add(at+k*beat,kick,.3)
            if k%2:
                tt=np.arange(int(.12*rate))/rate
                noise=rng.normal(0,1,len(tt)).astype(np.float32)
                add(at+k*beat,noise*np.exp(-tt*40),.065)
        for k in range(8):
            tt=np.arange(int(.045*rate))/rate
            noise=rng.normal(0,1,len(tt)).astype(np.float32)
            noise=np.concatenate(([0],np.diff(noise)))
            add(at+k*beat/2,noise*np.exp(-tt*100),.026,(-.35 if k%2 else .35))
    return mix


def build_timeline():
    manifest=json.loads((OUT/'audio/manifest.json').read_text(encoding='utf-8'))
    timeline=[];cursor=0
    minimum={'vi':8.0,'vim':10.5,'brand':4.8,'reveal':6.8,'end':7.5}
    for s in manifest['segments']:
        item=dict(s);duration=max(s['audio_seconds']+s['hold']+.4,minimum.get(s['scene'],0))
        item.update(start=round(cursor,6),duration=round(duration,6),voice_start=round(cursor+.2,6))
        timeline.append(item);cursor+=duration
    (OUT/'timeline.json').write_text(json.dumps(dict(title=manifest['title'],duration=cursor,segments=timeline),ensure_ascii=False,indent=2),encoding='utf-8')
    return timeline,cursor


def build_audio(timeline,duration):
    rate=44100;n=math.ceil(duration*rate)
    voice=np.zeros(n,np.float32);activity=np.zeros(n,np.float32)
    for s in timeline:
        sr,y=read_wav(s['audio'])
        y=np.interp(np.arange(int(len(y)*rate/sr))*sr/rate,np.arange(len(y)),y).astype(np.float32)
        # Target consistent RMS with a ceiling for natural dynamics.
        active=y[np.abs(y)>.025]
        rms=float(np.sqrt(np.mean(active**2))) if len(active) else .1
        y*=min(1.4,.16/max(rms,.001))
        start=int(s['voice_start']*rate);end=min(n,start+len(y))
        voice[start:end]+=y[:end-start]
        activity[max(0,start-int(.08*rate)):min(n,end+int(.18*rate))]=1
    music=synth_music(duration,rate)
    # Slowly duck the bed under narration, leaving the voice comfortably forward.
    step=441
    mask=activity[::step]
    mask=np.convolve(mask,np.ones(13)/13,mode='same')
    smooth=np.interp(np.arange(n),np.arange(len(mask))*step,mask)
    gain=.26*(1-.57*smooth)
    music*=gain[:,None]
    for s in timeline:
        if s['scene'] in ('brand','gnome','minecraft'):
            start=int(s['start']*rate);length=min(int(.18*rate),n-start)
            t=np.arange(length)/rate
            sweep=np.sin(2*np.pi*(1200*t-1800*t*t))*np.exp(-t*28)*.035
            music[start:start+length]+=sweep[:,None]
    fade=np.minimum(1,np.arange(n)/(rate*1.5))*np.minimum(1,(n-1-np.arange(n))/(rate*2.2))
    music*=fade[:,None]
    mix=music+voice[:,None]
    maximum=float(np.max(np.abs(mix)))
    if maximum>.94:mix*=.94/maximum
    write_wav(OUT/'audio/narration.wav',rate,voice)
    write_wav(OUT/'audio/original-music.wav',rate,music)
    write_wav(OUT/'audio/mix.wav',rate,mix)
    return dict(peak=float(np.max(np.abs(mix))),duration=duration,sample_rate=rate)


def subtitle_chunks(timeline):
    cues=[]
    for s in timeline:
        pieces=re.findall(r'[^，。？！]+[，。？！]?',s['text'])
        groups=[];current=''
        for p in pieces:
            if current and len(current+p)>31:groups.append(current);current=p
            else:current+=p
        if current:groups.append(current)
        total=sum(len(g) for g in groups);at=s['voice_start']
        for i,g in enumerate(groups):
            end=at+s['audio_seconds']*len(g)/total
            cues.append(dict(start=at,end=end+(.3 if i==len(groups)-1 else 0),text=g))
            at=end
    def stamp(t):
        ms=round(t*1000);return f'{ms//3600000:02d}:{ms//60000%60:02d}:{ms//1000%60:02d},{ms%1000:03d}'
    (OUT/'kinakaze-promo-v1.srt').write_text('\n\n'.join(f'{i+1}\n{stamp(c["start"])} --> {stamp(c["end"])}\n{c["text"]}' for i,c in enumerate(cues))+'\n',encoding='utf-8')
    return cues


class TerminalScreen(pyte.Screen):
    def __init__(self,*args,**kwargs):
        self.alt_saved=None
        super().__init__(*args,**kwargs)

    def set_mode(self,*modes,**kwargs):
        if kwargs.get('private') and any(m in (47,1047,1049) for m in modes):
            if self.alt_saved is None:
                self.alt_saved=(copy.deepcopy(self.buffer),copy.copy(self.cursor))
                self.erase_in_display(2)
                self.cursor_position(1,1)
            modes=tuple(m for m in modes if m not in (47,1047,1049))
        if modes:super().set_mode(*modes,**kwargs)

    def reset_mode(self,*modes,**kwargs):
        if kwargs.get('private') and any(m in (47,1047,1049) for m in modes):
            if self.alt_saved is not None:
                self.buffer,self.cursor=self.alt_saved
                self.alt_saved=None
                self.dirty.update(range(self.lines))
            modes=tuple(m for m in modes if m not in (47,1047,1049))
        if modes:super().reset_mode(*modes,**kwargs)

    def select_graphic_rendition(self,*attrs,**kwargs):
        # Vim emits private keyboard-reporting sequences ending in m. They
        # change the terminal protocol, not the visible character rendition.
        if not kwargs.get('private'):
            super().select_graphic_rendition(*attrs)


class Terminal:
    def __init__(self,name):
        self.data=json.loads((OUT/'captures'/f'{name}.json').read_text(encoding='utf-8'))
        self.events=self.data['events'];self.screen=TerminalScreen(92,22);self.stream=pyte.Stream(self.screen)
        self.index=0;self.last=-1;self.cached=None
        self.first=next((e[0] for e in self.events if 'kinakaze $ ' in e[2]),0)
        if name=='ssh':
            self.first=max(0,next((e[0] for e in self.events if 'CONNECTED TO' in e[2]),0)-.3)
        elif name=='java':
            clear_times=[e[0] for e in self.events if '\x1b[H\x1b[J' in e[2] or '\x1b[H\x1b[2J' in e[2]]
            if clear_times:self.first=clear_times[0]
        self.end=max(self.first+.1,self.data['duration']-.2)
        self.name=name

    def image(self,progress):
        t=self.first+min(.999,max(0,progress))*(self.end-self.first)
        if t<self.last:
            self.screen=TerminalScreen(92,22);self.stream=pyte.Stream(self.screen);self.index=0
        changed=False
        while self.index<len(self.events) and self.events[self.index][0]<=t:
            self.stream.feed(self.events[self.index][2]);self.index+=1;changed=True
        self.last=t
        if changed or self.cached is None:
            layer=Image.new('RGB',(1704,664),(12,19,34));d=ImageDraw.Draw(layer)
            d.rounded_rectangle((0,0,1703,663),20,outline=(65,94,122),width=2)
            for i,col in enumerate([PINK,(255,218,137),MINT]):d.ellipse((24+i*26,19,36+i*26,31),fill=col)
            label={'vi':'BusyBox Vi','vim':'Vim 9.0 / vim.tiny','java':'Linux JVM / JavaRuntimeProbe','ssh':'Windows OpenSSH  →  Linux sshd'}[self.name]
            text(d,(120,12),label,22,MUTED,mono=True)
            d.line((0,48,1704,48),fill=(41,58,78),width=1)
            for row,line in enumerate(self.screen.display):
                color=MINT if ('OK' in line or 'COMPLETE' in line or 'CONNECTED' in line) else WHITE
                text(d,(27,64+row*26),line.rstrip(),25,color,mono=True)
            cx,cy=self.screen.cursor.x,self.screen.cursor.y
            if not self.screen.cursor.hidden:
                d.rectangle((27+cx*13.75,64+cy*26+22,39+cx*13.75,64+cy*26+24),fill=MINT)
            self.cached=layer
        return self.cached


class Video:
    def __init__(self,path):
        self.cap=cv2.VideoCapture(str(path))
        if not self.cap.isOpened():raise FileNotFoundError(path)
        self.fps=self.cap.get(cv2.CAP_PROP_FPS);self.count=int(self.cap.get(cv2.CAP_PROP_FRAME_COUNT));self.index=-1;self.cached=None

    def image(self,t):
        target=min(self.count-1,max(0,int(t*self.fps)))
        if target!=self.index:
            if target!=self.index+1:self.cap.set(cv2.CAP_PROP_POS_FRAMES,target)
            ok,frame=self.cap.read()
            if not ok:raise RuntimeError('Could not decode footage')
            self.cached=Image.fromarray(cv2.cvtColor(frame,cv2.COLOR_BGR2RGB));self.index=target
        return self.cached


def background():
    yy,xx=np.mgrid[0:H,0:W]
    a=np.exp(-(((xx-1450)/950)**2+((yy-140)/700)**2))
    b=np.exp(-(((xx-100)/850)**2+((yy-940)/800)**2))
    rgb=np.stack([10+a*19+b*15,16+a*19+b*5,35+a*24+b*23],axis=2)
    return Image.fromarray(np.uint8(np.clip(rgb,0,255)))


def render(timeline,duration,cues,preview=False):
    base=background();videos={n:Video(OUT/'captures'/f'{n}.mp4') for n in ('minecraft','gnome','gnome-startup')}
    terminals={n:Terminal(n) for n in ('vi','vim','ssh','java')}
    groups={}
    for s in timeline:
        scene=s['scene']
        if scene not in groups:groups[scene]=[s['start'],s['start']+s['duration']]
        else:groups[scene][1]=s['start']+s['duration']
    starts=[s['start'] for s in timeline];cue_starts=[c['start'] for c in cues]
    labels={'hook':'WINDOWS × LINUX','guess':'换个角度，看熟悉的世界','reveal':'答案，和想象不同',
            'brand':'KINAKAZE','architecture':'让程序，跨越平台','vi':'从一行命令开始','vim':'熟悉的操作，在这里继续',
            'ssh':'连接，也在这里发生','desktop_guess':'把视野，再放大一点','gnome':'GNOME · 桌面亮起来了',
            'java':'JAVA · 更进一步','minecraft_guess':'下一个应用，会是什么？','minecraft':'MINECRAFT · 熟悉的世界',
            'montage':'从命令，到更多可能','outro':'开源，一起向前','end':'KINAKAZE'}
    target=OUT/'render/silent.mp4'
    cmd=['ffmpeg','-hide_banner','-loglevel','error','-y','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}',
         '-r',str(FPS),'-i','-','-an','-c:v','h264_nvenc','-preset','p5','-cq','20','-b:v','0',
         '-pix_fmt','yuv420p','-movflags','+faststart',str(target)]
    process=None if preview else subprocess.Popen(cmd,stdin=subprocess.PIPE,stderr=subprocess.PIPE)
    sample_times=[2,10,18,29,42,58,76,94,112,duration-4]
    if preview:
        indices=sorted(set(min(int(duration*FPS)-1,int(t*FPS)) for t in sample_times))
    else:indices=range(math.ceil(duration*FPS))
    sample_images=[]
    try:
        for index in indices:
            t=index/FPS;s=timeline[min(len(timeline)-1,bisect.bisect_right(starts,t)-1)]
            scene=s['scene'];local=t-s['start'];gs,ge=groups[scene];progress=(t-gs)/max(.001,ge-gs)
            im=base.copy();d=ImageDraw.Draw(im)
            # Sparse drifting particles and clean geometric lines supply motion between live shots.
            for p in range(22):
                x=(p*173+t*(7+p%4))%W;y=(p*107+math.sin(t*.4+p)*22)%H
                col=(45,64,83) if p%3 else (83,111,121)
                d.ellipse((x,y,x+3,y+3),fill=col)
            for offset in range(4):
                x=1180+offset*130+math.sin(t*.35)*35
                d.line((x,-40,x-750,H+40),fill=(34+offset*3,44,68),width=2)
            text(d,(78,39),'KINAKAZE',28,MINT,True)
            text(d,(1838,44),'Linux programs. New possibilities.',21,MUTED,anchor='ra')
            d.line((78,88,1842,88),fill=(65,82,108),width=1)
            rise=round(20*(1-ease(local/.55)))
            if scene in ('hook','guess','gnome'):
                source=videos['gnome-startup' if scene=='gnome' else 'gnome'].image((t-gs)+(3.9 if scene=='gnome' else 2))
                zoom=1+.018*ease(progress)
                shot=source.resize((round(1512*zoom),round(850*zoom)),Image.Resampling.LANCZOS)
                left=(shot.width-1512)//2;top=(shot.height-850)//2
                shot=shot.crop((left,top,left+1512,top+850))
                im.paste(shot,(204,124+rise));d=ImageDraw.Draw(im)
                d.rectangle((203,123+rise,1717,975+rise),outline=MINT,width=2)
                pill(d,(230,142+rise),'GNOME Shell · 启动实录' if scene=='gnome' else 'GNOME Shell · 启动画面')
                if scene=='hook' and local>1.8:
                    tx=1000+round(880*(1-ease((local-1.8)/.65)))
                    terminal=terminals['vi'].image(min(.65,local/9)).resize((780,304),Image.Resampling.LANCZOS)
                    im.paste(terminal,(tx,604));d=ImageDraw.Draw(im)
                    pill(d,(tx+14,551),'Vi · Linux 终端实录',PINK)
                if scene=='guess':
                    overlay=Image.new('RGBA',(W,H));od=ImageDraw.Draw(overlay)
                    od.rounded_rectangle((450,392,1470,612),28,fill=(10,17,36,225),outline=(*MINT,255),width=2)
                    text(od,(960,445),'你觉得，它运行在哪里？',58,WHITE,True,anchor='ma')
                    im=Image.alpha_composite(im.convert('RGBA'),overlay).convert('RGB');d=ImageDraw.Draw(im)
            elif scene in ('vi','vim','ssh','java'):
                text(d,(108,125+rise),labels[scene],61,WHITE,True)
                terminal=terminals[scene].image(progress)
                im.paste(terminal,(108,244+rise));d=ImageDraw.Draw(im)
                pill(d,(108,190+rise),'终端实录 / 剪辑',MINT)
                if scene=='ssh':pill(d,(1310,190),'Linux sshd · 密钥登录',PINK)
                elif scene=='java':pill(d,(1350,190),'JIT / 线程 / 文件 / 进程',PINK)
                else:pill(d,(1360,190),'Kinakaze · SSH 会话',PINK)
            elif scene=='minecraft':
                source=videos['minecraft'].image(.4+(t-gs))
                source=source.resize((1600,900),Image.Resampling.LANCZOS)
                im.paste(source,(160,105+rise));d=ImageDraw.Draw(im)
                pill(d,(184,126),'Minecraft · Linux 版客户端')
                text(d,(1775,899),'主菜单与按钮交互实录',23,WHITE,anchor='ra')
            elif scene=='architecture':
                text(d,(960,224+rise),'让熟悉的程序，遇见新的平台',65,WHITE,True,anchor='ma')
                for j,(label,sub) in enumerate([('Linux','x86-64 ELF 程序'),('Kinakaze','实验性兼容层'),('Windows','用户态运行')]):
                    x=160+j*560
                    d.rounded_rectangle((x,404,x+480,632),26,fill=(22,34,54),outline=MINT if j==1 else (75,96,123),width=3)
                    text(d,(x+240,445),label,58,MINT if j==1 else WHITE,True,anchor='ma')
                    text(d,(x+240,550),sub,28,MUTED,anchor='ma')
                    if j<2:
                        d.line((x+490,518,x+550,518),fill=PINK,width=3)
                        dot=x+490+((t*55)%60);d.ellipse((dot-5,513,dot+5,523),fill=PINK)
                pill(d,(510,736),'运行时无需管理员权限')
                pill(d,(1000,736),'无需自定义驱动',PINK)
            elif scene=='reveal':
                text(d,(140,208+rise),'你想到的，或许是 Linux。',49,MUTED)
                text(d,(140,350+rise),'这一次，是',70,WHITE,True)
                text(d,(136,430+rise),'Windows.',168,MINT,True)
                d.rectangle((148,663,148+int(1470*ease(local/1.1)),671),fill=PINK)
                text(d,(148,726),'同一台电脑，新的运行可能。',39,WHITE)
            elif scene in ('brand','outro','end'):
                text(d,(960,235+rise),'K I N A K A Z E',32,MINT,True,anchor='ma')
                text(d,(960,358+rise),'Kinakaze',153,WHITE,True,anchor='ma')
                tagline='让熟悉的程序，遇见新的可能。' if scene=='end' else '让 Linux 程序，在 Windows 上跑起来。'
                text(d,(960,594+rise),tagline,49,MINT,True,anchor='ma')
                d.line((550,699,1370,699),fill=PINK,width=2)
                if scene!='brand':
                    text(d,(960,751),'github.com/Kinakaze/Kinakaze',36,WHITE,anchor='ma',mono=True)
                    text(d,(960,822),'开源 · 持续开发 · 欢迎参与',26,MUTED,anchor='ma')
                    text(d,(960,869),'演示应用与依赖另行准备',23,MUTED,anchor='ma')
            elif scene in ('desktop_guess','minecraft_guess'):
                if scene=='desktop_guess':
                    big='几行命令之后';small='把视野，再放大一点。';tag='COMMAND LINE  →  DESKTOP'
                else:
                    big='下一个，会是什么？';small='从运行环境，走向熟悉的应用。';tag='JAVA  →  MORE POSSIBILITIES'
                text(d,(140,265+rise),tag,31,MINT,mono=True)
                text(d,(140,375+rise),big,96,WHITE,True)
                text(d,(145,560+rise),small,46,MUTED)
                # Flowing blocks are abstract motion graphics, with no countdown.
                for j in range(12):
                    x=200+j*134;y=744+math.sin(t*1.6+j*.5)*20
                    size=27+j%3*7;d.rectangle((x,y,x+size,y+size),fill=MINT if j%2 else PINK)
            elif scene=='montage':
                titles=['Vi / Vim','SSH / sshd','GNOME','Java','Minecraft']
                for j,label in enumerate(titles):
                    x=140+j*347;y=380+math.sin(t*1.8+j)*15
                    d.rounded_rectangle((x,y,x+306,y+220),24,fill=(24,38,58),outline=MINT if j%2==0 else PINK,width=2)
                    text(d,(x+153,y+79),label,38,WHITE,True,anchor='ma')
                text(d,(960,713),'从一行命令，到更多可能',59,WHITE,True,anchor='ma')
            # Consistent subtitle band, translucent without obscuring the actual action.
            overlay=Image.new('RGBA',(W,H));od=ImageDraw.Draw(overlay)
            od.rectangle((0,944,W,H),fill=(7,11,26,228))
            ci=bisect.bisect_right(cue_starts,t)-1
            if ci>=0 and t<=cues[ci]['end']:
                line=cues[ci]['text'];size=43 if len(line)<34 else 39
                text(od,(960,969),line,size,WHITE,True,anchor='ma')
            text(od,(80,1043),'KINAKAZE  /  DEVELOPMENT PREVIEW',18,MUTED,mono=True)
            if scene=='minecraft':footer='主菜单已验证 · 世界内游玩待验收'
            else:footer='VITS 女声配音  ·  AI 合成声音'
            text(od,(1840,1041),footer,20,MUTED,anchor='ra')
            od.rectangle((0,1076,int(W*t/duration),1079),fill=(*MINT,255))
            im=Image.alpha_composite(im.convert('RGBA'),overlay).convert('RGB')
            if t<.3:im=Image.blend(Image.new('RGB',(W,H),BG),im,ease(t/.3))
            if duration-t<.7:im=Image.blend(Image.new('RGB',(W,H),BG),im,max(0,(duration-t)/.7))
            if preview:
                image_path=OUT/'render'/f'preview-{index:05d}.jpg';im.save(image_path,quality=90)
                sample_images.append((t,im.resize((640,360))))
            else:
                process.stdin.write(im.tobytes())
                if index%(FPS*10)==0:print(f'Rendered {t:.0f} / {duration:.1f}s',flush=True)
        if preview:
            sheet=Image.new('RGB',(1280,math.ceil(len(sample_images)/2)*400),BG);sd=ImageDraw.Draw(sheet)
            for i,(at,shot) in enumerate(sample_images):
                x=(i%2)*640;y=(i//2)*400;sheet.paste(shot,(x,y));text(sd,(x+12,y+362),f'{at:.1f}s',23,WHITE)
            sheet.save(OUT/'render/contact-sheet.jpg',quality=90)
        else:
            process.stdin.close();err=process.stderr.read().decode(errors='replace');code=process.wait()
            if code:raise RuntimeError(err)
    finally:
        if process is not None and process.poll() is None:process.kill();process.wait()


def main():
    ap=argparse.ArgumentParser();ap.add_argument('--preview',action='store_true');args=ap.parse_args()
    timeline,duration=build_timeline();cues=subtitle_chunks(timeline)
    print('Duration',round(duration,2),flush=True)
    for name in ('vi','vim','ssh','java'):
        terminal=Terminal(name)
        terminal.image(.999)
    if not args.preview:
        report=build_audio(timeline,duration)
        (OUT/'reports/audio-mix.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
    render(timeline,duration,cues,args.preview)
    if not args.preview:
        subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-y','-i',str(OUT/'render/silent.mp4'),
                        '-i',str(OUT/'audio/mix.wav'),'-map','0:v','-map','1:a','-c:v','copy','-c:a','aac',
                        '-b:a','192k','-ar','48000','-movflags','+faststart','-shortest',
                        str(OUT/'kinakaze-promo-v1.mp4')],check=True)
        print('Finished:',OUT/'kinakaze-promo-v1.mp4',flush=True)


if __name__=='__main__':main()
