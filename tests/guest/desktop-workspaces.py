"""Exercise real cross-process window membership, overview and workspace actions."""
import argparse
import ctypes as c
import json
from pathlib import Path
import socket
import subprocess
import sys
import time
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'tools'))
from desktop_bridge import DesktopBridge
from session_process import SessionProcess

CHILD = '''
import ctypes as c,sys
gtk=c.CDLL('libgtk-3.so.0');g=c.CDLL('libgobject-2.0.so.0')
def bind(name,result,*args):
 f=getattr(gtk,name);f.restype=result;f.argtypes=args;return f
p=c.c_void_p;i=c.c_int
assert bind('gtk_init_check',i,p,p)(None,None)
new=bind('gtk_window_new',p,i);title=bind('gtk_window_set_title',None,p,c.c_char_p)
size=bind('gtk_window_set_default_size',None,p,i,i);show=bind('gtk_widget_show_all',None,p)
w=new(0);title(w,('Bridge '+sys.argv[1]).encode());size(w,360,240);show(w)
g.g_signal_connect_data.argtypes=[p,c.c_char_p,p,p,p,i]
callback=c.CFUNCTYPE(i,p,p,p)
@callback
def close(widget,event,data):gtk.gtk_main_quit();return 0
g.g_signal_connect_data(w,b'delete-event',close,None,None,0)
if sys.argv[1]=='Alpha':
 d=new(0);title(d,b'Bridge Modal');size(d,220,120);bind('gtk_window_set_transient_for',None,p,p)(d,w);show(d)
gtk.gtk_main()
'''
PARENT = 'import subprocess\np=[subprocess.Popen(["/usr/bin/python3.11","-c",'+repr(CHILD)+',name]) for name in ("Alpha","Beta")]\nfor child in p: child.wait()'

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist',required=True,type=Path)
    parser.add_argument('--root',required=True,type=Path)
    parser.add_argument('--report',required=True,type=Path)
    args=parser.parse_args();dist=args.dist.resolve();root=args.root.resolve()
    report=dict(status='failed',checks=[])
    command=[str(dist/'worker.exe'),'run','--root',str(root),'--dist',str(dist),'--','/usr/bin/python3.11','-c',PARENT]
    with args.report.with_suffix('.stderr.log').open('wb') as log:
        session=SessionProcess(command,stdout=log,stderr=log)
        directory=root/'tmp/desktop-workspaces-test'
        bridge=DesktopBridge(session.job,directory,'/tmp/desktop-workspaces-test');bridge.start()
        connection=socket.create_connection(('127.0.0.1',bridge.config['port']));connection.settimeout(40)
        stream=connection.makefile('r',encoding='utf-8')
        def send(**request):connection.sendall((json.dumps(request)+'\n').encode())
        def until(predicate,timeout=35):
            deadline=time.monotonic()+timeout
            while time.monotonic()<deadline:
                state=json.loads(stream.readline())
                if predicate(state):return state
            raise TimeoutError('Desktop state did not converge')
        def check(name,condition):
            assert condition,name
            report['checks'].append(name)
        try:
            send(token=bridge.token)
            state=until(lambda s:len(s.get('windows',[]))==3)
            items={item['title']:item for item in state['windows']}
            alpha,beta,modal=[int(items['Bridge '+name]['id']) for name in ('Alpha','Beta','Modal')]
            check('different application processes',items['Bridge Alpha']['pid']!=items['Bridge Beta']['pid'])
            send(op='move',id=str(alpha),index=1)
            state=until(lambda s:any(v['id']==str(alpha) and v['workspace']==1 for v in s.get('windows',[])))
            check('workspace hides parent and dialog',not bridge.user.IsWindowVisible(alpha) and not bridge.user.IsWindowVisible(modal) and bridge.user.IsWindowVisible(beta))
            check('dialog follows owner',next(v for v in state['windows'] if v['id']==str(modal))['workspace']==1)
            send(op='workspace',index=1)
            until(lambda s:s.get('workspace')==1)
            check('workspace restores parent and dialog',bridge.user.IsWindowVisible(alpha) and bridge.user.IsWindowVisible(modal) and not bridge.user.IsWindowVisible(beta))
            send(op='overview',enabled=True)
            state=until(lambda s:s.get('overview') is True)
            check('overview hides native windows',not any(bridge.user.IsWindowVisible(h) for h in (alpha,beta,modal)))
            check('overview retains all processes',len(state['windows'])==3)
            check('overview captures window preview',any(v.get('preview') for v in state['windows']))
            send(op='activate',id=str(beta))
            until(lambda s:s.get('workspace')==0 and s.get('overview') is False)
            check('activate switches workspace and restores target',bridge.user.IsWindowVisible(beta) and not bridge.user.IsWindowVisible(alpha))
            check('activate focuses the application',bridge.user.GetForegroundWindow()==beta)
            send(op='close',id='1')
            until(lambda s:s.get('type')=='error')
            check('foreign window actions rejected',True)
            send(op='close',id=str(beta))
            until(lambda s:s.get('type')=='state' and len(s['windows'])==2)
            check('close removes exited window',not bridge.user.IsWindow(beta))
            bridge.close()
            check('bridge shutdown restores hidden windows',bridge.user.IsWindowVisible(alpha) and bridge.user.IsWindowVisible(modal))
            report['status']='passed'
        except Exception as error:report['reason']=repr(error)
        finally:
            connection.close();bridge.close();session.close()
    args.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    return report['status']!='passed'

if __name__=='__main__':raise SystemExit(main())
