"""Verify a host tool invocation and a real shared guest-root file round trip."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'tools'))
from session_process import SessionProcess
OUT=ROOT/'artifacts/promo-v3'
GUEST=ROOT/'artifacts/promo-v1/guest-root'
DIST=ROOT/'artifacts/release-v0.1.0'
workspace=GUEST/'workspace/promo-agent';workspace.mkdir(parents=True,exist_ok=True)
input_file=workspace/'tasks.txt';input_file.write_text('windows\nlinux\nagent\nkinakaze\n',encoding='utf-8')
command="/bin/busybox sort /workspace/promo-agent/tasks.txt > /workspace/promo-agent/result.txt; /bin/busybox cat /workspace/promo-agent/result.txt"
args=[str(DIST/'worker.exe'),'run','--root',str(GUEST),'--dist',str(DIST),'--','/bin/sh','-ec',command]
started=time.monotonic()
job=SessionProcess(args,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
try:
    stdout,stderr=job.process.communicate(timeout=30)
    code=job.process.returncode
finally:job.close()
output_file=workspace/'result.txt';result=output_file.read_text(encoding='utf-8')
assert code==0 and result=='agent\nkinakaze\nlinux\nwindows\n',(code,result,stderr)
assert stdout.decode().replace('\r','')==result
report=dict(status='passed',tool='run_linux',invocation='Actual CLI subprocess invoked by the assistant',
            command=command,argv=args,exit_code=code,stdout=stdout.decode(),stderr=stderr.decode(errors='replace'),
            seconds=time.monotonic()-started,host_input=str(input_file),guest_input='/workspace/promo-agent/tasks.txt',
            host_output=str(output_file),host_readback=result,sha256=hashlib.sha256(output_file.read_bytes()).hexdigest(),
            scope='Tool adapter example using the CLI; shared files inside the prepared guest root. Not a bundled MCP server or arbitrary host mount.')
(OUT/'reports/agent-demo.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({k:v for k,v in report.items() if k in ('status','exit_code','stdout','host_readback','seconds')},ensure_ascii=False))
