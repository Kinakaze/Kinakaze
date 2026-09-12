"""Measure DWM preview pixels using two independent guest processes."""
import argparse,ctypes as c,importlib.util,json,struct,sys,time
from ctypes import wintypes as w
from pathlib import Path
from PIL import ImageGrab
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'tools'))
from session_process import SessionProcess
from desktop_bridge import DesktopBridge
spec=importlib.util.spec_from_file_location('repaint',Path(__file__).with_name('desktop-repaint.py'))
probe=importlib.util.module_from_spec(spec);spec.loader.exec_module(probe)
def main():
 p=argparse.ArgumentParser()
 for name in ('dist','root','report'):p.add_argument('--'+name,type=Path,required=True)
 a=p.parse_args();dist=a.dist.resolve();root=a.root.resolve()
 source=probe.GUEST.replace("0x2673b8)","(0xff0000 if sys.argv[1]=='destination' else 0x2673b8))")
 parent='import subprocess\np=[subprocess.Popen(["/usr/bin/python3.11","-c",'+repr(source)+',name]) for name in ("source","destination")]\nfor child in p:child.wait()'
 result={'status':'failed','checks':[]}
 with a.report.with_suffix('.log').open('wb') as log:
  session=SessionProcess([str(dist/'worker.exe'),'run','--root',str(root),'--dist',str(dist),'--','/usr/bin/python3.11','-c',parent],stdout=log,stderr=log)
  bridge=DesktopBridge(session.job,root/'tmp/dwm-probe','/tmp/dwm-probe')
  try:
   u=bridge.user;deadline=time.monotonic()+15
   while time.monotonic()<deadline:
    bridge._enumerate();items={r['title']:r for r in bridge.windows.values()}
    if all('Repaint '+n in items for n in ('source','destination')):break
    time.sleep(.03)
   src=items['Repaint source']['hwnd'];dst=items['Repaint destination']['hwnd']
   bridge.desktop=dst;u.SetPropW(dst,'KinakazeDesktopSurface',1)
   u.SetWindowPos(src,None,600,500,0,0,0x15)
   u.SetWindowPos(dst,-1,200,200,0,0,0x11)
   time.sleep(.4)
   assert bridge._thumbnails([dict(id=str(src),rect=[20,20,200,140])]),'DWM registration failed'
   u.ClientToScreen.argtypes=[w.HWND,c.POINTER(w.POINT)]
   point=w.POINT(80,80);u.ClientToScreen(dst,c.byref(point))
   def sample():return ImageGrab.grab().getpixel((point.x,point.y))
   deadline=time.monotonic()+2
   while sample()!=(0x26,0x73,0xb8) and time.monotonic()<deadline:time.sleep(.02)
   result['visible_pixel']=sample();assert sample()==(0x26,0x73,0xb8)
   result['checks'].append('DWM composed foreign frame')
   u.ShowWindow(src,0);time.sleep(.2)
   result['hidden_pixel']=sample()
   assert sample()==(0x26,0x73,0xb8),'hidden source lost its retained preview'
   result['checks'].append('DWM retains hidden source frame')
   u.ShowWindow(src,4)
   assert bridge._thumbnails([]);time.sleep(.2)
   result['cleared_pixel']=sample();assert sample()==(255,0,0)
   result['checks'].append('DWM unregister restores destination')
   result['status']='passed'
  finally:
   bridge.close();session.close();a.report.write_text(json.dumps(result,indent=2));print(json.dumps(result))
if __name__=='__main__':main()
