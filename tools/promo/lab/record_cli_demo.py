"""Execute the promotional CLI workflow and retain its actual output."""
from pathlib import Path
import hashlib,json,subprocess,sys,time

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools'))
from session_process import SessionProcess
OUT=ROOT/'artifacts/promo-amv';GUEST=ROOT/'artifacts/promo-v1/guest-root';DIST=ROOT/'artifacts/release-v0.1.0'
workspace=GUEST/'workspace/promo-cli';workspace.mkdir(parents=True,exist_ok=True)
source='windows\nlinux\nkinakaze\nlinux\n'
(workspace/'tasks.txt').write_text(source,encoding='utf-8')
records=[]
for command in ['cat tasks.txt','cat tasks.txt | sort -u > result.txt','cat result.txt']:
    argv=[str(DIST/'worker.exe'),'run','--root',str(GUEST),'--dist',str(DIST),'--','/bin/sh','-ec','cd /workspace/promo-cli; '+command]
    start=time.monotonic();job=SessionProcess(argv,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        stdout,stderr=job.process.communicate(timeout=20)
        assert job.process.returncode==0,(command,stderr)
        records.append({'command':command,'stdout':stdout.decode().replace('\r',''),'stderr':stderr.decode(),'exit_code':0,'seconds':time.monotonic()-start})
    finally:job.close()
result=(workspace/'result.txt').read_text(encoding='utf-8')
assert result=='kinakaze\nlinux\nwindows\n' and records[-1]['stdout']==result
report={'status':'passed','input':source,'commands':records,'host_readback':result,'host_file':str(workspace/'result.txt'),'guest_file':'/workspace/promo-cli/result.txt','scope':'Direct worker CLI invocations in the isolated demo root; actual commands, output, and file readback. Input timing is edited for the video.','sha256':hashlib.sha256((workspace/'result.txt').read_bytes()).hexdigest()}
(OUT/'reports/cli-demo-v3.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({'status':'passed','commands':[r['command'] for r in records],'readback':result},ensure_ascii=False))
