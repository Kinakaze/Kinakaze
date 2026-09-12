"""Counters and frame alarms cross real guest process boundaries, without polling."""
import ctypes as c,sys,subprocess,select,time
p,u,i=c.c_void_p,c.c_ulong,c.c_int
x=c.CDLL('libX11.so.6');s=c.CDLL('libXext.so.6')
def bind(lib,name,result,*args):
 f=getattr(lib,name);f.restype,f.argtypes=result,args;return f
class Value(c.Structure):_fields_=[('hi',i),('lo',c.c_uint)]
class Trigger(c.Structure):_fields_=[('counter',u),('type',i),('wait',Value),('test',i)]
class Alarm(c.Structure):_fields_=[('trigger',Trigger),('delta',Value),('events',i),('state',i)]
class Event(c.Structure):_fields_=[('type',i),('serial',u),('send',i),('display',p),('alarm',u),('counter',Value),('alarm_value',Value),('time',u),('state',i)]
d=bind(x,'XOpenDisplay',p,c.c_char_p)(None);assert d
create=bind(s,'XSyncCreateCounter',u,p,Value);update=bind(s,'XSyncSetCounter',i,p,u,Value)
close=bind(x,'XCloseDisplay',i,p)
if len(sys.argv)>1:
 counter=create(d,Value(0,2));print(counter,flush=True)
 for line in sys.stdin:
  if line.strip()=='close':break
  assert update(d,counter,Value(0,int(line)))
 close(d);sys.exit(0)
child=subprocess.Popen([sys.executable,__file__,'child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
try:
 foreign=int(child.stdout.readline());own=create(d,Value(0,1));assert own!=foreign,'counter XIDs collide across processes'
 val=Value();assert bind(s,'XSyncQueryCounter',i,p,u,p)(d,foreign,c.byref(val)) and val.lo==2
 attr=Alarm(Trigger(foreign,0,Value(0,4),2),Value(0,2),1,0)
 alarm=bind(s,'XSyncCreateAlarm',u,p,u,p)(d,63,c.byref(attr));assert alarm
 pending=bind(x,'XPending',i,p);next_event=bind(x,'XNextEvent',i,p,p)
 def events():
  found=[]
  while pending(d):
   data=(u*24)();next_event(d,data);event=c.cast(data,c.POINTER(Event)).contents
   if event.type==89 and event.alarm==alarm:found.append((event.counter.lo,event.alarm_value.lo,event.state))
  return found
 assert not events()
 fd=bind(x,'XConnectionNumber',i,p)(d)
 for value in (4,6):
  child.stdin.write(str(value)+'\n');child.stdin.flush()
  deadline=time.monotonic()+3;got=[]
  while time.monotonic()<deadline and not got:
   assert select.select([fd],[],[],max(0,deadline-time.monotonic()))[0],'counter update did not wake the X connection'
   got=events()
  assert got==[(value,value,0)],got
 child.stdin.write('close\n');child.stdin.flush();assert child.wait(timeout=5)==0
 deadline=time.monotonic()+3;got=[]
 while time.monotonic()<deadline and not got:
  select.select([fd],[],[],max(0,deadline-time.monotonic()));got=events()
 assert got and got[0][2]==1,'closing the counter owner did not disable its foreign alarm'
 bind(s,'XSyncDestroyAlarm',i,p,u)(d,alarm);bind(s,'XSyncDestroyCounter',i,p,u)(d,own)
 close(d)
 print('SYNC_FOREIGN_COUNTER_FRAME_ALARM_WAKEUP_OK',flush=True)
finally:
 if child.poll() is None:child.kill();child.wait()
