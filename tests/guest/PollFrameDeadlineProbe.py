"""Measure actual short poll sleeps used by desktop frame clocks."""
import os,select,time,statistics
fd=os.eventfd(0,os.EFD_NONBLOCK);p=select.poll();p.register(fd,select.POLLIN)
for timeout in (1,2,8,16):
 samples=[]
 for _ in range(40):
  start=time.perf_counter();assert not p.poll(timeout);samples.append((time.perf_counter()-start)*1000)
 print('POLL_DEADLINE',timeout,'median_ms',round(statistics.median(samples),3),'p95_ms',round(sorted(samples)[37],3),flush=True)
os.close(fd)
