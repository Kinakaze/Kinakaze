"""Record Shell's actual startup frames in the isolated promo environment."""
from pathlib import Path
import subprocess
import sys
import time
import psutil
from inspect_windows import windows

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v1'
owner=OUT/'reports/desktop-owner.pid'
old_windows={(w['hwnd'],w['pid']) for w in windows()}
if owner.exists():
    try:
        p=psutil.Process(int(owner.read_text()))
        if any(arg.replace('\\','/').endswith('tools/promo/launch_desktop.py') for arg in p.cmdline()):
            p.terminate();p.wait(10)
    except psutil.NoSuchProcess:
        pass
with (OUT/'reports/gnome-startup-record.log').open('wb') as log:
    p=subprocess.Popen([sys.executable,str(ROOT/'tools/promo/launch_desktop.py'),
                        '--dist','artifacts/release-v0.1.0','--root','artifacts/promo-v1/gnome-root',
                        '--timeout','110','--report','artifacts/promo-v1/reports/gnome-startup-session.json'],
                       cwd=ROOT,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
    (OUT/'reports/desktop-startup-owner.pid').write_text(str(p.pid))
    deadline=time.monotonic()+60
    while time.monotonic()<deadline:
        found=[w for w in windows() if w['title']=='Kinakaze GNOME Shell' and (w['hwnd'],w['pid']) not in old_windows]
        if found:break
        if p.poll() is not None:raise RuntimeError('GNOME exited during startup')
        time.sleep(.1)
    else:raise RuntimeError('No Shell window')
    subprocess.run([sys.executable,str(ROOT/'tools/promo/capture_window.py'),'--hwnd',str(found[0]['hwnd']),
                    '--output',str(OUT/'captures/gnome-startup.mp4'),'--seconds','30','--desktop-demo'],
                   cwd=ROOT,check=True)
