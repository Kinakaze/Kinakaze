"""Bounded Linux browser startup/page probes with fresh, disposable profiles."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
import uuid
from session_process import SessionProcess


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--dist',type=Path,required=True)
    parser.add_argument('--output-dir',type=Path,required=True)
    parser.add_argument('--only',action='append',choices=['abi','firefox-version','chrome-version','firefox-glxtest','firefox-page','chrome-page'])
    parser.add_argument('--timeout',type=float,default=40)
    parser.add_argument('--chrome-no-sandbox',action='store_true',help='diagnostic run in a fresh profile; never changes browser defaults')
    parser.add_argument('--session-bus',action='store_true',help='run page probes under a private dbus-run-session')
    parser.add_argument('--elf-imports',type=Path,default=Path('target/release/elf-imports'))
    parser.add_argument('--page-file',type=Path,help='custom page retaining the JS marker and screenshot background contract')
    parser.add_argument('--chrome-arg',action='append',default=[],help='extra Chrome page diagnostic argument; recorded in the report')
    args=parser.parse_args()
    if not 0<args.timeout<=180:parser.error('timeout must be within (0, 180]')
    root,dist,out=(p.resolve() for p in (args.root,args.dist,args.output_dir))
    guest='/tmp/browser-runtime-'+uuid.uuid4().hex
    stage=root/guest.lstrip('/')
    stage.mkdir(parents=True);out.mkdir(parents=True,exist_ok=True)
    for folder in ['home','runtime','firefox-profile','chrome-profile']:(stage/folder).mkdir()
    (stage/'index.html').write_text('<!doctype html><html><head><title>Kinakaze browser test</title>'
        '<style>html,body{margin:0;background:#000;color:white}h1{margin:100px 40px}</style>'
        '</head><body><h1 id="result">WAIT</h1><script>document.getElementById("result").textContent="KINAKAZE_BROWSER_JS_"+(6*7);'
        'document.documentElement.style.background="#193b57";document.body.style.background="#193b57";</script></body></html>',encoding='utf-8')
    probes=Path(__file__).resolve().parents[1]/'tests/guest'
    if args.page_file:
        (stage/'index.html').write_bytes(args.page_file.read_bytes())
    selected=args.only or ['abi','firefox-version','chrome-version','firefox-glxtest','firefox-page','chrome-page']
    if 'abi' in selected:
        (stage/'BrowserStartupAbiProbe.py').write_bytes((probes/'BrowserStartupAbiProbe.py').read_bytes())
        (stage/'BrowserXcbSetupProbe.py').write_bytes((probes/'BrowserXcbSetupProbe.py').read_bytes())
        subprocess.run(['clang','--target=x86_64-linux-gnu','-fuse-ld=lld','-fPIC','-shared','-nostdlib',
                        '-fno-builtin','-ffp-model=strict','-O2','-Wl,--strip-all',str(probes/'BrowserStartupAbiProbe.c'),'-o',str(stage/'BrowserStartupAbiProbe.so')],check=True)
        subprocess.run(['clang','--target=x86_64-linux-gnu','-fuse-ld=lld','-fPIC','-shared','-nostdlib',
                        '-fno-builtin','-ffp-model=strict','-O2','-Wl,--no-eh-frame-hdr',str(probes/'BrowserStartupAbiProbe.c'),
                        '-o',str(stage/'BrowserStartupAbiProbe-no-header.so')],check=True)
        subprocess.run(['clang','--target=x86_64-linux-gnu','-fuse-ld=lld','-fPIE','-pie','-nostdlib',
                        '-fno-builtin','-O2','-Wl,--dynamic-linker=/lib64/ld-linux-x86-64.so.2',
                        str(probes/'BrowserForkTlsProbe.c'),'-L'+str(args.elf_imports.resolve()),
                        '-l:libc.so.6','-l:libpthread.so.0','-o',str(stage/'BrowserForkTlsProbe')],check=True)
    cases={
        'abi':(['/usr/bin/python3.11',guest+'/BrowserStartupAbiProbe.py'],'BROWSER_STARTUP_ABI_OK'),
        'firefox-version':(['/opt/firefox/firefox','--version'],'Mozilla Firefox'),
        'chrome-version':(['/opt/google/chrome/chrome','--version'],'Google Chrome'),
        'firefox-glxtest':(['/opt/firefox/glxtest'],'TEST_TYPE\nGLX'),
        'firefox-page':(['/opt/firefox/firefox','--headless','--no-remote','--profile',guest+'/firefox-profile',
                         '--window-size','800,600','--screenshot',guest+'/firefox.png','file://'+guest+'/index.html'],None),
        'chrome-page':(['/opt/google/chrome/chrome','--headless','--no-first-run','--disable-background-networking',
                        *(['--no-sandbox'] if args.chrome_no_sandbox else []),'--user-data-dir='+guest+'/chrome-profile',
                        *args.chrome_arg,'--dump-dom','file://'+guest+'/index.html'],'<h1 id="result">KINAKAZE_BROWSER_JS_42</h1>'),
    }
    def sha(path):
        with path.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
    report=dict(root=str(root),dist=str(dist),staging=str(stage),chrome_no_sandbox=args.chrome_no_sandbox,session_bus=args.session_bus,
                page_sha256=sha(stage/'index.html'),
                images={p.name:sha(p) for p in (dist/'rootfs/lib').iterdir() if p.is_file()},
                worker_sha256=sha(dist/'worker.exe'),results=[])
    for name in selected:
        command,marker=cases[name]
        if args.session_bus and name.endswith('-page'):
            command=['/usr/bin/dbus-run-session','--',*command]
        logpath=out/(name+'.log')
        result=dict(id=name,argv=command,status='timeout',log=str(logpath))
        start=time.monotonic()
        environment=['/usr/bin/env','HOME='+guest+'/home','XDG_RUNTIME_DIR='+guest+'/runtime',
                     '/bin/sh','-c','/bin/busybox chmod 700 "$XDG_RUNTIME_DIR" && exec "$@"','browser-probe']
        with logpath.open('wb') as log:
            child=SessionProcess([str(dist/'worker.exe'),'run','--root',str(root),'--dist',str(dist),
                                  '--',*environment,*command],stdout=log,stderr=log)
            try:
                result['exit_code']=child.process.wait(timeout=args.timeout)
                result['status']='passed' if result['exit_code']==0 else 'failed'
            except subprocess.TimeoutExpired:pass
            finally:child.close()
        if marker:
            result['marker_found']=marker.encode() in logpath.read_bytes()
            if result['status']=='passed' and not result['marker_found']:result['status']='failed'
        if name=='firefox-page':
            screenshot=stage/'firefox.png'
            result['screenshot_exists']=screenshot.is_file()
            if result['status']=='passed' and not screenshot.is_file():result['status']='failed'
            if screenshot.is_file():
                (out/'firefox.png').write_bytes(screenshot.read_bytes())
                from PIL import Image
                with Image.open(screenshot) as picture:
                    result['screenshot_size']=list(picture.size)
                    result['background_matches']=picture.convert('RGB').getpixel((40,40))==(25,59,87)
                    result['javascript_completed']=result['background_matches']
                if not result['background_matches']:result['status']='failed'
        result['elapsed_seconds']=round(time.monotonic()-start,3)
        report['results'].append(result)
        report['summary']=dict(Counter(row['status'] for row in report['results']))
        temporary=out/'results.json.tmp'
        temporary.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');temporary.replace(out/'results.json')
        print(json.dumps(result),flush=True)
    return int(any(row['status']!='passed' for row in report['results']))


if __name__=='__main__':raise SystemExit(main())
