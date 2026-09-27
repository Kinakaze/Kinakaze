"""A light Japanese editorial motion film built around recorded software footage."""
import argparse
import bisect
import functools
import html
import json
import math
from pathlib import Path
import re
import subprocess
import wave

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont
import render as source

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v2'
RAW=ROOT/'artifacts/promo-v1'
W,H,FPS=1920,1080,60
PAPER=(246,250,252)
INK=(33,52,68)
BLUE=(70,137,183)
MUTED=(114,137,153)
LINE=(208,225,236)
WHITE=(255,255,255)
BEAT=60/175

@functools.lru_cache(maxsize=100)
def font(size,weight=400,face='sans'):
    names={'sans':'NotoSansSC-VF.ttf','serif':'NotoSerifSC-VF.ttf','latin':'calibril.ttf','mono':'consola.ttf'}
    f=ImageFont.truetype('C:/Windows/Fonts/'+names[face],size)
    if face in ('sans','serif'):f.set_variation_by_axes([weight])
    return f

def txt(im,xy,value,size=36,color=INK,weight=400,anchor=None,face='sans'):
    ImageDraw.Draw(im).text(xy,value,font=font(size,weight,face),fill=color,anchor=anchor)

def clamp(x):return min(1,max(0,x))
def ease(x):return 1-(1-clamp(x))**4
def smooth(x):x=clamp(x);return x*x*(3-2*x)
def lerp(a,b,v):return a+(b-a)*v

@functools.lru_cache(maxsize=1)
def bg():
    yy,xx=np.mgrid[0:H,0:W].astype(np.float32)
    a=np.exp(-((xx-1580)/750)**2-((yy-120)/650)**2)
    b=np.exp(-((xx-200)/850)**2-((yy-1000)/470)**2)
    ar=np.stack([248-36*a-7*b,251-15*a-3*b,252-2*a],axis=2)
    return Image.fromarray(ar.astype(np.uint8))

@functools.lru_cache(maxsize=1)
def glass():
    """Original shaded torus, composited as a quiet translucent spatial accent."""
    size=900;y,x=np.mgrid[-1:1:complex(size),-1:1:complex(size)].astype(np.float32)
    x=x+.14*y;r=np.sqrt(x*x+(y*1.12)**2)
    q=(r-.63)/.17;valid=np.abs(q)<1
    z=np.sqrt(np.maximum(0,1-q*q));nx=q*x/np.maximum(r,.01);ny=q*y/np.maximum(r,.01)
    light=np.clip(-.35*nx-.6*ny+.9*z,0,1)
    spec=light**16
    color=np.stack([112+110*light+28*spec,172+70*light+16*spec,207+38*light+10*spec],axis=2)
    alpha=valid*(.32+.5*z)*220
    ar=np.dstack([np.clip(color,0,255),alpha]).astype(np.uint8)
    return Image.fromarray(ar)

@functools.lru_cache(maxsize=32)
def shadow(size,radius=24):
    w,h=size;im=Image.new('RGBA',(w+100,h+100));d=ImageDraw.Draw(im)
    d.rounded_rectangle((45,48,w+55,h+58),radius,fill=(63,102,126,38))
    return im.filter(ImageFilter.GaussianBlur(19))

@functools.lru_cache(maxsize=32)
def rounded_mask(size,radius):
    mask=Image.new('L',size);ImageDraw.Draw(mask).rounded_rectangle((0,0,size[0]-1,size[1]-1),radius,fill=255)
    return mask

def frame(im,content,box,radius=20):
    x,y,w,h=map(round,box)
    sh=shadow((w,h),radius);im.paste(sh,(x-50,y-50),sh)
    content=content.resize((w,h),Image.Resampling.LANCZOS)
    im.paste(content,(x,y),rounded_mask((w,h),radius))
    ImageDraw.Draw(im).rounded_rectangle((x,y,x+w-1,y+h-1),radius,outline=(220,232,240),width=1)

def label(im,x,y,value):
    d=ImageDraw.Draw(im);d.line((x,y+14,x+35,y+14),fill=BLUE,width=2)
    txt(im,(x+53,y),value,21,BLUE,500)

def mark(im,x,y,size=40):
    d=ImageDraw.Draw(im);u=size/40
    d.polygon([(x,y+4*u),(x+8*u,y),(x+8*u,y+40*u),(x,y+36*u)],fill=BLUE)
    d.polygon([(x+12*u,y+19*u),(x+32*u,y),(x+41*u,y),(x+23*u,y+20*u),(x+41*u,y+40*u),(x+31*u,y+40*u)],fill=(138,186,215))

def chrome(im,section):
    mark(im,80,54,26);txt(im,(126,46),'kinakaze',30,INK,face='latin')
    txt(im,(1840,55),section,18,MUTED,anchor='ra',face='mono')

class Terminal(source.Terminal):
    def __init__(self,name):
        super().__init__(name)
        # Edit past the SSH login and command-typing pre-roll, preserving the recorded editor actions.
        if name=='vi':
            self.first=next(e[0] for e in self.events if '\x1b[?1049h' in e[2])
        elif name=='vim':
            self.first=next(e[0] for e in self.events if '3L, 55B' in e[2])
        elif name=='java':
            self.first=max(self.first,next(e[0] for e in self.events if 'openjdk version' in e[2])-.15)

    def image(self,progress):
        super().image(progress)
        signature=tuple(self.screen.display)+(self.screen.cursor.x,self.screen.cursor.y)
        if getattr(self,'custom_sig',None)==signature:return self.custom_image
        self.custom_sig=signature
        im=Image.new('RGB',(1518,610),(20,32,43));d=ImageDraw.Draw(im)
        d.rectangle((0,0,1518,48),fill=(28,43,56))
        for i in range(3):d.ellipse((22+i*22,20,29+i*22,27),fill=(122,151,169))
        names={'vi':'BusyBox vi','vim':'Vim 9.0','ssh':'Windows OpenSSH  →  Linux sshd','java':'Linux Java / JavaRuntimeProbe'}
        txt(im,(92,8),names[self.name],20,(192,209,221),face='mono')
        for row,line in enumerate(self.screen.display):
            color=(166,220,247) if any(w in line for w in ('OK','COMPLETE','CONNECTED')) else (226,233,238)
            txt(im,(28,57+row*24),line.rstrip(),24,color,face='mono')
        if not self.screen.cursor.hidden:
            x=28+self.screen.cursor.x*14.4;y=58+self.screen.cursor.y*24
            d.rectangle((x,y+21,x+13,y+23),fill=(155,212,242))
        self.custom_image=im
        return im

def timeline():
    path=OUT/'audio/manifest.json'
    if path.exists():manifest=json.loads(path.read_text(encoding='utf-8'))
    else:
        manifest=json.loads((ROOT/'tools/promo/script-v2.json').read_text(encoding='utf-8'))
        for s in manifest['segments']:s['audio_seconds']=len(s['text'])/5.4
    mins={'hook':6.8,'reveal':4.1,'brand':5.5,'vi':6.9,'vim':8.2,'ssh':8.2,'desktop_guess':5.5,'gnome':11.,'java':10.9,
          'minecraft_guess':5.5,'montage':6.9,'outro':8.2,'end':8.0}
    result=[];cursor=0
    for s in manifest['segments']:
        length=max(s['audio_seconds']+s['hold']+.45,mins.get(s['scene'],0))
        length=math.ceil(length/(BEAT*2))*BEAT*2
        item=dict(s,start=cursor,duration=length,voice_start=cursor+.32)
        result.append(item);cursor+=length
    # Preserve breathing room for menu footage and the closing musical phrase.
    if cursor<127.9:
        extra=127.9-cursor
        result[-1]['duration']+=min(3,extra)
        if extra>3:
            idx=next(i for i,s in enumerate(result) if s['id']=='14')
            result[idx]['duration']+=extra-3
            for s in result[idx+1:]:s['start']+=extra-3;s['voice_start']+=extra-3
        cursor=127.9
    data=dict(title=manifest['title'],duration=cursor,segments=result)
    (OUT/'timeline.json').write_text(json.dumps(data,ensure_ascii=False,indent=2),encoding='utf-8')
    return result,cursor

def subtitles(tl):
    cues=[]
    for s in tl:
        pieces=re.findall(r'[^，。？！]+[，。？！]?',s['text']);groups=[];cur=''
        for p in pieces:
            if cur and len(cur+p)>24:groups.append(cur);cur=''
            cur+=p
        if cur:groups.append(cur)
        total=sum(len(g) for g in groups);at=s['voice_start']
        for g in groups:
            end=at+s['audio_seconds']*len(g)/total
            cues.append(dict(start=at,end=end,text=g));at=end
    def stamp(t):
        n=round(t*1000);return f'{n//3600000:02}:{n//60000%60:02}:{n//1000%60:02},{n%1000:03}'
    (OUT/'kinakaze-promo-v2.srt').write_text('\n\n'.join(f'{i+1}\n{stamp(s["start"])} --> {stamp(s["end"])}\n{s["text"]}' for i,s in enumerate(cues)),encoding='utf-8')
    return cues

def audio_mix(tl,duration):
    rate=48000;n=math.ceil(duration*rate);voice=np.zeros(n,np.float32);active=np.zeros(n,np.float32)
    for s in tl:
        sr,y=source.read_wav(s['audio'])
        y=np.interp(np.arange(int(len(y)*rate/sr))*sr/rate,np.arange(len(y)),y).astype(np.float32)
        rms=np.sqrt(np.mean(y[np.abs(y)>.02]**2));y*=min(2,.18/max(rms,.01))
        a=int(s['voice_start']*rate);b=min(n,a+len(y));voice[a:b]+=y[:b-a]
        active[max(0,a-int(.15*rate)):min(n,b+int(.20*rate))]=1
    musicfile=OUT/'assets/summer-triangle.mp3'
    raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(musicfile),'-f','f32le','-ac','2','-ar',str(rate),'-'])
    track=np.frombuffer(raw,dtype='<f4').reshape(-1,2)
    if len(track)<n:
        # Repeat a full musical phrase only if new narration exceeds the original song.
        track=np.concatenate([track,track[int(32*BEAT*rate):int(96*BEAT*rate)]])
    music=track[:n].copy()
    mask=active[::480];mask=np.convolve(mask,np.ones(23)/23,mode='same')
    duck=np.interp(np.arange(n),np.arange(len(mask))*480,mask)
    music*=((.44-.31*duck)*np.minimum(1,np.arange(n)/(.6*rate))*np.minimum(1,(n-1-np.arange(n))/(2.5*rate)))[:,None]
    mixed=voice[:,None]+music
    source.write_wav(OUT/'audio/narration.wav',rate,voice)
    source.write_wav(OUT/'audio/premix.wav',rate,mixed)
    subprocess.run(['ffmpeg','-v','error','-y','-i',str(OUT/'audio/premix.wav'),'-af',
                    'highpass=f=55,loudnorm=I=-16:TP=-1.5:LRA=9','-ar','48000',str(OUT/'audio/mix.wav')],check=True)

class Film:
    def __init__(self,tl,duration,cues):
        self.tl=tl;self.duration=duration;self.cues=cues
        self.starts=[s['start'] for s in tl];self.cstarts=[c['start'] for c in cues]
        self.groups={}
        for s in tl:
            if s['scene'] not in self.groups:self.groups[s['scene']]=[s['start'],s['start']+s['duration']]
            else:self.groups[s['scene']][1]=s['start']+s['duration']
        self.terms={n:Terminal(n) for n in ('vi','vim','ssh','java')}
        self.videos={n:source.Video(RAW/'captures'/f'{n}.mp4') for n in ('gnome','gnome-startup','minecraft')}

    def paper(self,t):
        im=bg().copy();d=ImageDraw.Draw(im)
        # Restrained drifting horizon, not a field of particles or repeated UI cards.
        offset=math.sin(t*.17)*22
        d.arc((1070+offset,-330,2140+offset,740),70,240,fill=(206,226,238),width=1)
        d.arc((1110+offset,-290,2100+offset,700),80,245,fill=(223,237,246),width=1)
        return im

    def scene(self,t):
        idx=max(0,bisect.bisect_right(self.starts,t)-1);s=self.tl[idx]
        kind=s['scene'];gs,ge=self.groups[kind];local=t-gs;progress=clamp(local/(ge-gs));enter=ease(local/.95)
        im=self.paper(t);rise=int(24*(1-enter));d=ImageDraw.Draw(im)
        if kind=='hook':
            chrome(im,'A FAMILIAR WORLD')
            label(im,100,218,'换个角度，看熟悉的世界')
            txt(im,(94,280+rise),'这个桌面，',82,weight=500)
            txt(im,(94,387+rise),'运行在哪里？',82,weight=500)
            txt(im,(100,537),'A familiar world.\nA different possibility.',32,MUTED,face='latin')
            v=self.videos['gnome'].image(2+local)
            frame(im,v,(770+32*(1-enter),186,1066,600))
            if local>1.25:
                e=ease((local-1.25)/.8)
                terminal=self.terms['vi'].image(clamp((local-1)/6))
                frame(im,terminal,(635,630+45*(1-e),870,350),14)
            txt(im,(1835,845),'GNOME / 启动画面',19,MUTED,anchor='ra')
        elif kind=='reveal':
            chrome(im,'THE REVEAL')
            txt(im,(130,202),'你想到的，或许是 Linux。',36,MUTED)
            txt(im,(121,309+rise),'这一次，是',65,weight=400)
            txt(im,(118,407+rise),'Windows.',184,BLUE,face='latin')
            d.line((130,658,130+int(995*ease(local/1.2)),658),fill=(148,189,215),width=2)
            txt(im,(133,706),'熟悉的程序，新的运行平台。',31,MUTED)
            ring=glass().resize((750,750));im.paste(ring,(1170,100+int(math.sin(local)*10)),ring)
            for j in range(4):
                x=1430+(j%2)*93;y=360+(j//2)*93
                d.rectangle((x,y,x+78,y+78),fill=(255,255,255))
        elif kind in ('brand','outro','end'):
            ring=glass().resize((1000,1000));im.paste(ring,(1050,-320+int(math.sin(t*.5)*15)),ring)
            ring2=glass().resize((490,490));im.paste(ring2,(-250,675),ring2)
            if kind=='brand':
                txt(im,(960,247),'MEET THE PROJECT',22,BLUE,anchor='ma',face='mono')
                mark(im,861,328+rise,150)
                txt(im,(960,505+rise),'Kinakaze',160,INK,anchor='ma',face='latin')
                txt(im,(960,727),'让 Linux 程序，在 Windows 上运行。',38,MUTED,anchor='ma')
            else:
                label(im,155,213,'开源，一起向前')
                txt(im,(143,298+rise),'Kinakaze',164,INK,face='latin')
                txt(im,(155,524),'让熟悉的程序，',55,weight=400)
                txt(im,(155,599),'遇见新的可能。',55,BLUE,weight=500)
                d.line((159,731,1215,731),fill=LINE,width=1)
                txt(im,(155,775),'github.com/Kinakaze/Kinakaze',34,INK,face='latin')
                txt(im,(157,838),'实验性项目  /  欢迎体验与参与',24,MUTED)
                if kind=='end':
                    txt(im,(1837,799),'MUSIC  SUMMER TRIANGLE',18,MUTED,anchor='ra',face='mono')
                    txt(im,(1837,836),'しゃろう / Sharou',20,MUTED,anchor='ra')
                    txt(im,(1837,871),'VITS · AI 合成配音',20,MUTED,anchor='ra')
        elif kind=='architecture':
            chrome(im,'LINUX PROGRAMS / WINDOWS')
            label(im,116,174,'从这里，跨越平台')
            txt(im,(109,240),'熟悉的程序，换个地方打开。',69,weight=500)
            xs=[162,752,1320]
            for j,(a,b) in enumerate([('Linux','x86-64 ELF'),('Kinakaze','实验性兼容层'),('Windows','用户态运行')]):
                delay=j*.25;dy=int((1-ease((local-delay)/.8))*35)
                txt(im,(xs[j],447+dy),a,90,BLUE if j==1 else INK,face='latin')
                txt(im,(xs[j]+4,575+dy),b,25,MUTED,face='mono' if j==0 else 'sans')
                if j<2:
                    x=xs[j]+376;d.line((x,514,x+107,514),fill=(164,194,214),width=2)
                    dot=x+(local*60)%107;d.ellipse((dot-4,510,dot+4,518),fill=BLUE)
            d.line((116,687,1797,687),fill=LINE,width=1)
            txt(im,(162,735),'无需管理员权限',30,INK)
            txt(im,(838,735),'无需自定义驱动',30,INK)
        elif kind in ('vi','vim','ssh','java'):
            titles={'vi':('从一行命令开始','vi','打开文件 / 输入内容'),
                    'vim':('把想法写下来','Vim','编辑 / 保存 / 回到终端'),
                    'ssh':('连接，在这里发生','SSH','Windows 客户端 → Linux sshd'),
                    'java':('再往前一步','Java','Linux 版运行环境')}
            tag,title,desc=titles[kind];chrome(im,'RUNNING ON KINAKAZE');label(im,92,134,tag)
            txt(im,(86,184+rise),title,88,INK,face='latin')
            txt(im,(1818,242),desc,24,MUTED,anchor='ra')
            term=self.terms[kind].image(progress)
            frame(im,term,(120,296+rise,1680,675),20)
            # Leave active terminal content unobstructed by positioning captions in white space.
            if kind=='java':
                for j,(lab,key) in enumerate([('JIT','JAVA_JIT_THREADS_FILES_OK'),('线程 / 文件','JAVA_JIT_THREADS_FILES_OK'),('子进程','JAVA_SPAWN_OK')]):
                    done=key in ''.join(self.terms[kind].screen.display)
                    txt(im,(880+j*305,179),('✓  ' if done else '·  ')+lab,24,BLUE if done else MUTED)
        elif kind=='desktop_guess':
            chrome(im,'BEYOND THE TERMINAL')
            txt(im,(118,239+rise),'不止于',83,weight=400)
            txt(im,(117,337+rise),'一行命令。',105,BLUE,500)
            txt(im,(123,528),'把视野，再放大一点。',32,MUTED)
            term=self.terms['vim'].image(.68)
            frame(im,term,(1060-local*19,230,1040,419),18)
            v=self.videos['gnome-startup'].image(5)
            frame(im,v,(879-local*12,563,1024,576),18)
        elif kind=='gnome':
            chrome(im,'DESKTOP / STARTUP CAPTURE')
            txt(im,(94,144),'GNOME',86,INK,face='latin')
            txt(im,(1830,171),'Linux 桌面，在 Windows 上亮起来。',31,MUTED,anchor='ra')
            v=self.videos['gnome-startup'].image(3.85+local)
            # Camera framing is an editorial move; the recorded desktop is not fabricated.
            width=1330+int(12*smooth(progress));height=int(width*9/16)
            frame(im,v,((W-width)/2,218+rise,width,height),20)
            label(im,102,305,'启动实录')
        elif kind=='minecraft_guess':
            chrome(im,'WHAT COMES NEXT')
            label(im,134,205,'从运行环境，走向熟悉的应用')
            txt(im,(125,282+rise),'下一个，',92,weight=400)
            txt(im,(123,411+rise),'你想到了什么？',92,BLUE,500)
            txt(im,(132,634),'A new possibility is opening.',33,MUTED,face='latin')
            # Voxel fragments assemble without a countdown or character illustration.
            for j in range(7):
                x=1330+(j%3)*130;y=240+(j//3)*152+math.sin(local+j)*15
                u=68;d.polygon([(x,y),(x+u,y-32),(x+2*u,y),(x+u,y+32)],fill=(202,230,246))
                d.polygon([(x,y),(x+u,y+32),(x+u,y+118),(x,y+84)],fill=(134,187,222))
                d.polygon([(x+u,y+32),(x+2*u,y),(x+2*u,y+84),(x+u,y+118)],fill=(176,214,240))
        elif kind=='minecraft':
            v=self.videos['minecraft'].image(.4+local)
            # The actual welcome click and moving title panorama lead this chapter.
            scale=1.0+.018*smooth(progress);vw=round(W*scale);vh=round(H*scale)
            im=v.resize((vw,vh),Image.Resampling.LANCZOS).crop(((vw-W)//2,(vh-H)//2,(vw+W)//2,(vh+H)//2))
            overlay=Image.new('RGBA',(W,H));od=ImageDraw.Draw(overlay)
            od.rectangle((0,0,W,90),fill=(16,26,36,165));im.paste(overlay,(0,0),overlay)
            txt(im,(64,21),'MINECRAFT  /  LINUX CLIENT',26,WHITE,face='mono')
            txt(im,(1855,24),'主菜单与按钮交互实录',23,(222,233,240),anchor='ra')
            txt(im,(1856,916),'世界内游玩仍在推进',22,WHITE,anchor='ra')
        elif kind=='montage':
            chrome(im,'FROM ONE COMMAND TO MORE')
            txt(im,(101,149),'从一行命令，到更多可能。',62,weight=500)
            v=self.videos['gnome'].image(10)
            frame(im,v,(95,316+int(math.sin(local)*8),850,478),16)
            v=self.videos['minecraft'].image(9+local)
            frame(im,v,(982,413+int(math.sin(local+1)*8),846,476),16)
            frame(im,self.terms['java'].image(.97),(468,778,764,308),14)
            txt(im,(100,834),'Vi / Vim  ·  SSH  ·  GNOME',24,MUTED,face='latin')
        return im

    def image(self,t):
        im=self.scene(t)
        idx=max(0,bisect.bisect_right(self.starts,t)-1);s=self.tl[idx];elapsed=t-s['start']
        # Real scene transitions use an eased lateral dissolve; adjacent narration stays continuous.
        if idx>0 and s['scene']!=self.tl[idx-1]['scene'] and elapsed<.45:
            e=smooth(elapsed/.45);previous=self.scene(max(0,s['start']-1/FPS))
            shifted=Image.new('RGB',(W,H),PAPER);shifted.paste(im,(round(35*(1-e)),0))
            im=Image.blend(previous,shifted,e)
        ci=bisect.bisect_right(self.cstarts,t)-1
        if ci>=0 and t<self.cues[ci]['end']:
            value=self.cues[ci]['text'];size=38
            dark=s['scene']=='minecraft';overlay=Image.new('RGBA',(W,H));d=ImageDraw.Draw(overlay)
            width=d.textlength(value,font=font(size,500))+66
            # Soft white capsule keeps the voice legible without a permanent lower-third block.
            color=(15,28,39,190) if dark else (250,253,255,246)
            d.rounded_rectangle(((W-width)/2,978,(W+width)/2,1052),18,fill=color)
            txt(overlay,(960,989),value,size,WHITE if dark else INK,500,anchor='ma')
            im.paste(overlay,(0,0),overlay)
        if self.duration-t<.8:im=Image.blend(Image.new('RGB',(W,H),PAPER),im,clamp((self.duration-t)/.8))
        return im

def render_movie(film,path,seconds=None):
    duration=min(film.duration,seconds) if seconds else film.duration
    cmd=['ffmpeg','-v','error','-y','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r',str(FPS),'-i','-',
         '-an','-c:v','h264_nvenc','-preset','p5','-cq','18','-b:v','0','-pix_fmt','yuv420p',str(path)]
    p=subprocess.Popen(cmd,stdin=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        for i in range(math.ceil(duration*FPS)):
            p.stdin.write(film.image(i/FPS).tobytes())
            if i%(FPS*5)==0:print('Rendered',i/FPS,'/',round(duration,2),flush=True)
        p.stdin.close();error=p.stderr.read();code=p.wait()
        if code:raise RuntimeError(error.decode(errors='replace'))
    finally:
        if p.poll() is None:p.kill();p.wait()

def package(tl,duration):
    names={'hook':'开场','reveal':'揭晓','vi':'Vi / Vim','ssh':'SSH','gnome':'GNOME','java':'Java','minecraft':'Minecraft','end':'片尾'}
    seen=set();buttons=[]
    for s in tl:
        if s['scene'] in names and s['scene'] not in seen:
            seen.add(s['scene']);buttons.append(f'<button data-time="{s["start"]}">{names[s["scene"]]}</button>')
    page='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Kinakaze / 清透日系新版</title><style>*{box-sizing:border-box}body{margin:0;background:#f4f8fb;color:#213444;font:16px/1.7 system-ui,"Microsoft YaHei",sans-serif}main{width:min(1240px,92vw);margin:42px auto}small{color:#5985a3;letter-spacing:.22em}h1{font-size:42px;line-height:1.35;font-weight:500;margin:12px 0 32px}video{width:100%;display:block;border-radius:18px;box-shadow:0 24px 70px #31547222}nav,.links{display:flex;flex-wrap:wrap;gap:12px;margin:24px 0}button,a{font:inherit;cursor:pointer;text-decoration:none;color:#416c88;background:white;padding:8px 17px;border:1px solid #d9e5ed;border-radius:9px}p,footer{color:#7a8e9d}footer{font-size:13px;margin-top:36px;padding-top:24px;border-top:1px solid #dbe6ed}</style>
<main><small>KINAKAZE / FILM 02</small><h1>看起来是 Linux。<br>运行在 Windows。</h1><video controls preload="metadata" poster="poster.jpg" src="kinakaze-promo-v2.mp4"></video><nav>'''+''.join(buttons)+f'''</nav><p>{duration:.1f} 秒 · 1080p / 60 fps · 清透日系 · VITS 合成女声</p><div class="links"><a href="kinakaze-promo-v2.mp4" download>下载新版</a><a href="kinakaze-promo-v2.srt" download>字幕</a><a href="../promo-v1/kinakaze-promo-v1.mp4">查看上一版</a></div><footer>Music: SUMMER TRIANGLE / しゃろう（Sharou） · OpenTracks（旧 DOVA-SYNDROME）<br>GNOME 为启动展示，Minecraft 为主菜单与按钮交互实录。使用 AI 合成配音。</footer></main><script>const v=document.querySelector('video');document.querySelectorAll('[data-time]').forEach(b=>b.onclick=()=>{{v.currentTime=+b.dataset.time;v.play()}})</script></html>'''
    (OUT/'preview.html').write_text(page,encoding='utf-8')

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--preview',action='store_true');ap.add_argument('--seconds',type=float);args=ap.parse_args()
    tl,duration=timeline();cues=subtitles(tl);film=Film(tl,duration,cues)
    print('Duration',duration,flush=True)
    if args.preview:
        samples=[s['start']+min(2.1,s['duration']*.6) for s in tl if s['id'] not in ('05','14')]
        sheet=Image.new('RGB',(1440,math.ceil(len(samples)/2)*441),PAPER)
        for i,t in enumerate(samples):
            shot=film.image(t);shot.save(OUT/'render'/f'frame-{i:02}.jpg',quality=94)
            x=i%2*720;y=i//2*441;sheet.paste(shot.resize((720,405)),(x,y));txt(sheet,(x+14,y+405),f'{t:.1f} s',18,MUTED,face='mono')
        sheet.save(OUT/'render/contact-sheet.jpg',quality=93)
        film.image(tl[2]['start']+2.4).save(OUT/'poster.jpg',quality=95)
    else:
        assert (OUT/'audio/manifest.json').exists(),'Synthesize the new narration first.'
        audio_mix(tl,duration);render_movie(film,OUT/'render/silent.mp4',args.seconds)
        subprocess.run(['ffmpeg','-v','error','-y','-i',str(OUT/'render/silent.mp4'),'-i',str(OUT/'audio/mix.wav'),
                        '-map','0:v','-map','1:a','-c:v','copy','-c:a','aac','-b:a','256k','-ar','48000','-shortest',
                        '-movflags','+faststart',str(OUT/'kinakaze-promo-v2.mp4')],check=True)
    package(tl,duration)

if __name__=='__main__':main()
