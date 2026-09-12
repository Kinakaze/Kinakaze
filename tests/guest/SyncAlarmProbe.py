"""Exercise counter crossings, alarm rearming, timer counters and fence waits."""
import ctypes as c,time,threading
p,u,i=c.c_void_p,c.c_ulong,c.c_int
x=c.CDLL('libX11.so.6');s=c.CDLL('libXext.so.6')
def bind(lib,name,result,*args):
 f=getattr(lib,name);f.restype,f.argtypes=result,args;return f
class Value(c.Structure):
 _fields_=[('hi',i),('lo',c.c_uint)]
 def number(self):return (self.hi<<32)|self.lo
def val(v):return Value(v>>32,v&0xffffffff)
class Trigger(c.Structure):_fields_=[('counter',u),('type',i),('wait',Value),('test',i)]
class Alarm(c.Structure):_fields_=[('trigger',Trigger),('delta',Value),('events',i),('state',i)]
class Event(c.Structure):_fields_=[('type',i),('serial',u),('send',i),('display',p),('alarm',u),('counter',Value),('alarm_value',Value),('time',u),('state',i)]
class SystemCounter(c.Structure):_fields_=[('name',c.c_char_p),('counter',u),('resolution',Value)]
d=bind(x,'XOpenDisplay',p,c.c_char_p)(None);assert d
major,minor=i(),i();assert bind(s,'XSyncInitialize',i,p,c.POINTER(i),c.POINTER(i))(d,c.byref(major),c.byref(minor))
assert (major.value,minor.value)==(3,1)
counter=bind(s,'XSyncCreateCounter',u,p,Value)(d,val(0));assert counter
attributes=Alarm(Trigger(counter,0,val(7),2),val(5),1,0)
alarm=bind(s,'XSyncCreateAlarm',u,p,u,c.POINTER(Alarm))(d,63,c.byref(attributes));assert alarm
pending=bind(x,'XPending',i,p);next_event=bind(x,'XNextEvent',i,p,p)
def events():
 out=[]
 while pending(d):
  b=(u*24)();next_event(d,b);e=c.cast(b,c.POINTER(Event)).contents
  if e.type==89:out.append((e.alarm,e.counter.number(),e.alarm_value.number(),e.state))
 return out
set_counter=bind(s,'XSyncSetCounter',i,p,u,Value)
assert set_counter(d,counter,val(6));assert not events()
assert set_counter(d,counter,val(7));assert events()==[(alarm,7,7,0)]
assert set_counter(d,counter,val(20));assert events()==[(alarm,20,12,0)]
query_alarm=bind(s,'XSyncQueryAlarm',i,p,u,c.POINTER(Alarm));result=Alarm();assert query_alarm(d,alarm,c.byref(result));assert result.trigger.wait.number()==22
destroy_alarm=bind(s,'XSyncDestroyAlarm',i,p,u);assert destroy_alarm(d,alarm);assert events()[0][-1]==2
assert bind(s,'XSyncDestroyCounter',i,p,u)(d,counter)
count=i();cs=bind(s,'XSyncListSystemCounters',c.POINTER(SystemCounter),p,c.POINTER(i))(d,c.byref(count));assert cs and count.value==2
assert {cs[n].name for n in range(count.value)}=={b'SERVERTIME',b'IDLETIME'}
clock=next(cs[n].counter for n in range(count.value) if cs[n].name==b'SERVERTIME')
now=Value();assert bind(s,'XSyncQueryCounter',i,p,u,c.POINTER(Value))(d,clock,c.byref(now))
attributes=Alarm(Trigger(clock,0,val(now.number()+50),2),val(0),1,0)
alarm=bind(s,'XSyncCreateAlarm',u,p,u,c.POINTER(Alarm))(d,63,c.byref(attributes));assert alarm
time.sleep(.1);assert any(e[0]==alarm and e[-1]==1 for e in events());destroy_alarm(d,alarm);events()
bind(s,'XSyncFreeSystemCounterList',None,c.POINTER(SystemCounter))(cs)
w=bind(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)(d,1,0,0,8,8,0,0,0)
fence=bind(s,'XSyncCreateFence',u,p,u,i)(d,w,0);assert fence
query=bind(s,'XSyncQueryFence',i,p,u,c.POINTER(i));triggered=i();assert query(d,fence,c.byref(triggered)) and triggered.value==0
await_fence=bind(s,'XSyncAwaitFence',i,p,c.POINTER(u),i);finished=[]
t=threading.Thread(target=lambda:finished.append(await_fence(d,(u*1)(fence),1)));t.start();time.sleep(.03);assert not finished
assert bind(s,'XSyncTriggerFence',i,p,u)(d,fence);t.join(2);assert finished==[1]
assert query(d,fence,c.byref(triggered)) and triggered.value==1
assert bind(s,'XSyncResetFence',i,p,u)(d,fence);assert query(d,fence,c.byref(triggered)) and triggered.value==0
bind(s,'XSyncDestroyFence',i,p,u)(d,fence);bind(x,'XDestroyWindow',i,p,u)(d,w);bind(x,'XCloseDisplay',i,p)(d)
print('SYNC_COUNTER_ALARM_TIMER_FENCE_OK',flush=True)
