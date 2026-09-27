"""Capture real ConPTY output as timestamped terminal events for the video."""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import socket
import subprocess
import sys
import threading
import time

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'artifacts/promo-v1'
sys.path.insert(0,str(OUT/'python'))
sys.path.insert(0,str(ROOT/'tools'))
from winpty import PtyProcess
from session_process import SessionProcess

DIST=ROOT/'artifacts/release-v0.1.0'
GUEST=OUT/'guest-root'
WORKER=DIST/'worker.exe'
JAVA='/usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java'


def capture(name, command, actions, tail=2):
    process=PtyProcess.spawn(command,cwd=str(ROOT),dimensions=(22,92))
    start=time.monotonic()
    events=[]
    def reader():
        while True:
            try:
                data=process.read(8192)
                if not data:break
                events.append([round(time.monotonic()-start,4),'o',data])
            except (EOFError,OSError):break
    reader_thread=threading.Thread(target=reader,daemon=True)
    reader_thread.start()
    if name != 'ssh':
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            if any('kinakaze $ ' in event[2] for event in events):break
            if not process.isalive():raise RuntimeError('Interactive SSH session ended before the shell prompt')
            time.sleep(.05)
        else:raise RuntimeError('Interactive SSH shell prompt did not arrive')
        if name=='java':
            process.write('/bin/busybox clear\r')
            time.sleep(.5)
    for delay,text,typed in actions:
        time.sleep(delay)
        if typed:
            for char in text:
                process.write(char)
                time.sleep(.045)
        else:
            process.write(text)
    time.sleep(tail)
    duration=time.monotonic()-start
    try:
        process.write('exit\r')
        time.sleep(.5)
        process.close(force=True)
    except (EOFError,OSError):pass
    report=dict(version=2,width=92,height=22,duration=round(duration,4),
                title=name,command=command,source='Actual Kinakaze process through Windows ConPTY',events=events)
    path=OUT/'captures'/(name+'.json')
    path.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    print(name,round(duration,2),'seconds',len(events),'events',flush=True)
    return report


def shell():
    setup=f"export TERM=xterm-256color; export PS1='kinakaze $ '; export JAVA={JAVA}; exec /bin/sh -i"
    return [str(WORKER),'run','--root',str(GUEST),'--dist',str(DIST),'--','/bin/sh','-c',setup]


def ssh(tui=None):
    fixtures=GUEST/'root/.ssh/promo-video'
    fixtures.mkdir(parents=True,exist_ok=True)
    guest_path='/root/.ssh/promo-video'
    for name in ('host','client'):
        if not (fixtures/name).exists():
            subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(fixtures/name)],check=True,capture_output=True)
    (fixtures/'authorized_keys').write_bytes((fixtures/'client.pub').read_bytes())
    with socket.socket() as s:
        s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    config=f'''Port {port}
ListenAddress 127.0.0.1
HostKey {guest_path}/host
PidFile {guest_path}/sshd.pid
AuthorizedKeysFile {guest_path}/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
UsePAM no
StrictModes yes
LogLevel INFO
PrintLastLog no
'''
    (fixtures/'sshd_config').write_text(config,encoding='utf-8',newline='\n')
    setup=f'/bin/busybox mkdir -p /run/sshd; /bin/busybox chmod 755 / /run /run/sshd; /bin/busybox chmod 700 /root /root/.ssh {guest_path}; /bin/busybox chmod 600 {guest_path}/host {guest_path}/authorized_keys; exec /usr/sbin/sshd -D -e -f {guest_path}/sshd_config'
    log=OUT/'reports/sshd.log'
    with log.open('wb') as stream:
        session=SessionProcess([str(WORKER),'run','--root',str(GUEST),'--dist',str(DIST),'--','/bin/sh','-ec',setup],stdout=stream,stderr=stream)
        try:
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                if 'Server listening' in log.read_text(errors='replace'):break
                if session.process.poll() is not None:raise RuntimeError(log.read_text(errors='replace'))
                time.sleep(.2)
            else:raise RuntimeError('sshd did not listen')
            base=['ssh','-T','-F','NUL','-p',str(port),'-i',str(fixtures/'client'),
                  '-o','IdentitiesOnly=yes','-o','BatchMode=yes','-o','LogLevel=ERROR','-o','StrictHostKeyChecking=accept-new',
                  '-o',f'UserKnownHostsFile={fixtures/"known_hosts"}','root@127.0.0.1']
            if tui:
                interactive=base.copy()
                interactive[1]='-tt'
                remote=f"export TERM=xterm-256color; export PS1='kinakaze $ '; export JAVA={JAVA}; exec /bin/sh -i"
                capture(tui,interactive+[remote],actions_for(tui),tail=5 if tui=='java' else 2)
                return
            remote="printf 'CONNECTED TO LINUX SSHD\\n'; /bin/busybox id; printf 'windows\\nlinux\\nkinakaze\\n' | /bin/busybox sort; printf 'PIPELINE COMPLETE\\n'"
            result=subprocess.run(base+[remote],capture_output=True,timeout=20)
            (OUT/'reports/ssh-result.json').write_text(json.dumps(dict(returncode=result.returncode,stdout=result.stdout.decode(errors='replace')),indent=2),encoding='utf-8')
            if result.returncode:raise RuntimeError(result.stderr.decode(errors='replace'))
            # This clip transparently labels the host client; actual remote output is recorded.
            capture('ssh',base+[remote],[],tail=8)
        finally:
            session.close()
            session.process.wait(timeout=10)


def actions_for(mode):
    if mode=='vi':
        return [(1.2,'vi /tmp/hello.txt\r',True),(.8,'i',False),
                             (.4,'Hello, Linux.\rWelcome to Windows.\rPowered by Kinakaze.',True),
                             (1.0,'\x1b',False),(.3,':wq\r',True),(.7,'cat /tmp/hello.txt\r',True)]
    if mode=='vim':
        return [(1.2,'/usr/bin/vim.tiny -Nu NONE /tmp/hello.txt\r',True),(.8,'G',False),
                              (.4,'o',False),(.3,'Linux tools. New possibilities.',True),
                              (.8,'\x1b',False),(.3,':wq\r',True),(.5,'cat /tmp/hello.txt\r',True)]
    return [(1.2,'$JAVA -version\r',True),(1.5,'$JAVA -cp /tests/java JavaRuntimeProbe spawn\r',True)]


def main():
    ap=argparse.ArgumentParser();ap.add_argument('mode',choices=['vi','vim','java','ssh']);args=ap.parse_args()
    if args.mode=='vi':
        (GUEST/'tmp/hello.txt').write_text('',encoding='utf-8')
    if args.mode=='java':
        out=GUEST/'tests/java';out.mkdir(parents=True,exist_ok=True)
        subprocess.run(['E:/APPD/JDK23/bin/javac.exe','--release','17','-d',str(out),str(ROOT/'tests/guest/JavaRuntimeProbe.java')],check=True)
    ssh(None if args.mode=='ssh' else args.mode)


if __name__=='__main__':main()
