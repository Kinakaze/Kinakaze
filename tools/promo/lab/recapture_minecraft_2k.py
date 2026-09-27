"""Record a borderless client inside the 1920x1080 physical capture surface."""
from pathlib import Path
import subprocess,sys,time,json,ctypes as c
import psutil
from PIL import ImageGrab

ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-amv'
sys.path.insert(0,str(ROOT/'tools/promo'))
from inspect_windows import windows,user

log=(OUT/'reports/minecraft-hd.log').open('wb')
launch=[sys.executable,str(ROOT/'tools/run-minecraft.py'),'--root',str(ROOT/'artifacts/promo-v1/guest-root'),'--dist',str(ROOT/'artifacts/release-v0.1.0'),'--worker',str(ROOT/'artifacts/release-v0.1.0/worker.exe'),'--width','1920','--height','1080']
before={(w['hwnd'],w['pid']) for w in windows()}
process=subprocess.Popen(launch,cwd=ROOT,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
owned=psutil.Process(process.pid)
report={'launcher_pid':process.pid,'requested_size':[1920,1080],'reason':'Full 2K window exceeded the physical desktop and was clipped; capture the complete 1080p client instead.'}
try:
    deadline=time.monotonic()+70
    while time.monotonic()<deadline:
        hits=[w for w in windows() if 'minecraft' in w['title'].lower() and (w['hwnd'],w['pid']) not in before]
        if hits:break
        if process.poll() is not None:raise RuntimeError('Demo launch exited; see minecraft-hd.log')
        time.sleep(.2)
    else:raise RuntimeError('No new Minecraft window')
    win=hits[0];hwnd=win['hwnd'];report['window']=win
    # Keep this recording window from accepting accidental desktop clicks.
    user.EnableWindow.argtypes=[c.c_void_p,c.c_bool]
    user.EnableWindow(hwnd,False)
    user.GetWindowLongW.argtypes=[c.c_void_p,c.c_int];user.GetWindowLongW.restype=c.c_long
    user.SetWindowLongW.argtypes=[c.c_void_p,c.c_int,c.c_long]
    style=user.GetWindowLongW(hwnd,-16)
    user.SetWindowLongW(hwnd,-16,(style & ~0x00CF0000)|0x80000000)
    user.MoveWindow(hwnd,0,0,1920,1080,True)
    time.sleep(20)
    image=ImageGrab.grab(window=hwnd).convert('RGB');report['captured_size']=list(image.size)
    image.save(OUT/'reports/minecraft-hd-before.png')
    assert image.size==(1920,1080),image.size
    fps=30;seconds=14
    target=OUT/'public/minecraft-hd.mp4'
    encoder=subprocess.Popen(['ffmpeg','-v','error','-y','-f','rawvideo','-pix_fmt','rgb24','-video_size','1920x1080','-framerate',str(fps),'-i','-','-an','-c:v','h264_nvenc','-preset','p7','-cq','15','-pix_fmt','yuv420p','-movflags','+faststart',str(target)],stdin=subprocess.PIPE,stderr=subprocess.PIPE)
    start=time.monotonic()
    try:
        for frame in range(fps*seconds):
            wait=start+frame/fps-time.monotonic()
            if wait>0:time.sleep(wait)
            shot=ImageGrab.grab(window=hwnd).convert('RGB')
            assert shot.size==(1920,1080)
            encoder.stdin.write(shot.tobytes())
        encoder.stdin.close();err=encoder.stderr.read().decode(errors='replace');assert encoder.wait()==0,err
    finally:
        if encoder.poll() is None:encoder.kill();encoder.wait()
    report.update(status='recorded',frames=fps*seconds,fps=fps,wall_seconds=time.monotonic()-start)
    ImageGrab.grab(window=hwnd).save(OUT/'reports/minecraft-hd-after.png')
    print(json.dumps(report),flush=True)
finally:
    # Terminate only the process tree started by this capture script.
    try:children=owned.children(recursive=True)
    except psutil.NoSuchProcess:children=[]
    for child in reversed(children):
        try:child.terminate()
        except psutil.NoSuchProcess:pass
    try:owned.terminate()
    except psutil.NoSuchProcess:pass
    psutil.wait_procs(children+[owned],timeout=4)
    log.close();(OUT/'reports/minecraft-hd.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
