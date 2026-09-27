"""Render deterministic WebGL frames and composite typography into a 1080p film."""
import argparse
from functools import partial
from http.server import ThreadingHTTPServer,SimpleHTTPRequestHandler
import json
import math
from pathlib import Path
import subprocess
import threading
import time
from urllib.parse import unquote,urlsplit
from PIL import Image,ImageDraw
from playwright.sync_api import sync_playwright

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v3'
FPS=60

class Handler(SimpleHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_GET(self):
        path=Path(self.translate_path(self.path))
        if path.suffix=='.mp4' and path.is_file():
            length=path.stat().st_size;range_header=self.headers.get('Range')
            begin,end=0,length-1
            if range_header:
                values=range_header.replace('bytes=','').split('-');begin=int(values[0] or 0)
                if values[1]:end=min(end,int(values[1]))
            self.send_response(206 if range_header else 200)
            self.send_header('Content-Type','video/mp4');self.send_header('Accept-Ranges','bytes')
            self.send_header('Content-Length',str(end-begin+1))
            if range_header:self.send_header('Content-Range',f'bytes {begin}-{end}/{length}')
            self.end_headers()
            try:
                with path.open('rb') as f:
                    f.seek(begin);remaining=end-begin+1
                    while remaining:
                        data=f.read(min(remaining,1024*1024));self.wfile.write(data);remaining-=len(data)
            except (ConnectionResetError,ConnectionAbortedError,BrokenPipeError):pass
            return
        super().do_GET()
    def translate_path(self,path):
        path=unquote(urlsplit(path).path)
        mounts={'/three/':OUT/'runtime/node_modules/three','/assets/':OUT/'assets',
                '/captures/':ROOT/'artifacts/promo-v1/captures','/fonts/':Path('C:/Windows/Fonts')}
        for prefix,base in mounts.items():
            if path.startswith(prefix):
                dest=(base/path[len(prefix):]).resolve()
                if dest.is_relative_to(base.resolve()):return str(dest)
        if path in ('/keynote.html','/keynote.js'):return str(ROOT/'tools/promo'/path[1:])
        return str(OUT/'file-not-found')

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--preview',action='store_true');ap.add_argument('--seconds',type=float);ap.add_argument('--start',type=float,default=0);args=ap.parse_args()
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
    errors=[];frames=0;start_wall=time.monotonic()
    try:
        with sync_playwright() as pw:
            def new_browser():
                b=pw.chromium.launch(headless=True,args=['--use-angle=d3d11','--force-high-performance-gpu','--disable-background-timer-throttling','--autoplay-policy=no-user-gesture-required'])
                p=b.new_page(viewport={'width':1920,'height':1080},device_scale_factor=1)
                p.on('pageerror',lambda e:errors.append(str(e)))
                p.goto(f'http://127.0.0.1:{server.server_port}/keynote.html',wait_until='load',timeout=90000)
                try:p.wait_for_function('window.filmReady === true',timeout=90000)
                except Exception:
                    print('PAGE_ERRORS',errors,flush=True);raise
                return b,p
            browser,page=new_browser()
            gl=page.evaluate('''()=>{const g=document.querySelector('canvas').getContext('webgl2');const e=g.getExtension('WEBGL_debug_renderer_info');return e?g.getParameter(e.UNMASKED_RENDERER_WEBGL):g.getParameter(g.RENDERER)}''')
            print('GPU',gl,flush=True)
            if args.preview:
                samples=[3,10,20,25.5,31.8,40,46,52,58,64.5,72,83,92.8,98.5,107,115,124]
                sheet=Image.new('RGB',(1440,math.ceil(len(samples)/2)*426),'#eef6fb')
                for i,t in enumerate(samples):
                    page.evaluate('(t)=>window.renderAt(t)',t)
                    path=OUT/'render'/f'still-{i:02}.jpg';page.screenshot(path=str(path),type='jpeg',quality=96)
                    im=Image.open(path);x=i%2*720;y=i//2*426;sheet.paste(im.resize((720,405)),(x,y));ImageDraw.Draw(sheet).text((x+12,y+408),f'{t:.1f} s',fill='#3b5d74')
                sheet.save(OUT/'render/contact-sheet.jpg',quality=94)
                page.evaluate('(t)=>window.renderAt(t)',25.7);page.screenshot(path=str(OUT/'poster.jpg'),type='jpeg',quality=97)
            else:
                duration=min(args.seconds or 127.9,127.9-args.start)
                target=OUT/'render'/('sample-silent.mp4' if args.seconds else 'silent.mp4')
                command=['ffmpeg','-v','error','-y','-f','image2pipe','-vcodec','mjpeg','-r',str(FPS),'-i','-',
                         '-an','-c:v','h264_nvenc','-preset','p5','-cq','17','-b:v','0','-pix_fmt','yuv420p',str(target)]
                proc=subprocess.Popen(command,stdin=subprocess.PIPE,stderr=subprocess.PIPE)
                try:
                    for i in range(math.ceil(duration*FPS)):
                        # Periodically release browser/GPU capture resources. Every frame is
                        # a pure function of its timestamp, so restarts do not change motion.
                        if i and i%(FPS*8)==0:
                            browser.close();browser,page=new_browser()
                        for attempt in range(3):
                            try:
                                page.evaluate('(t)=>window.renderAt(t)',args.start+i/FPS)
                                shot=page.screenshot(type='jpeg',quality=95,timeout=12000)
                                break
                            except Exception:
                                if attempt==2:raise
                                print(f'Restarting capture at frame {i}',flush=True)
                                browser.close();browser,page=new_browser()
                        proc.stdin.write(shot)
                        frames+=1
                        if i%(FPS*2)==0:print(f'{i/FPS:.0f}/{duration:.1f}s | {frames/(time.monotonic()-start_wall):.1f} fps export',flush=True)
                    proc.stdin.close();err=proc.stderr.read();code=proc.wait()
                    if code:raise RuntimeError(err.decode(errors='replace'))
                finally:
                    if proc.poll() is None:proc.kill();proc.wait()
                output=OUT/('motion-sample.mp4' if args.seconds else 'kinakaze-promo-v3.mp4')
                subprocess.run(['ffmpeg','-v','error','-y','-i',str(target),'-ss',str(args.start),'-i',str(OUT/'audio/mix.wav'),
                                '-map','0:v','-map','1:a','-c:v','copy','-c:a','aac','-b:a','256k','-ar','48000',
                                '-shortest','-movflags','+faststart',str(output)],check=True)
            (OUT/'reports'/('browser-preview.json' if args.preview else 'browser-render.json')).write_text(json.dumps(dict(errors=errors,gpu=gl,frames=frames,wall_seconds=time.monotonic()-start_wall),ensure_ascii=False,indent=2),encoding='utf8')
            assert not errors,errors
            browser.close()
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
