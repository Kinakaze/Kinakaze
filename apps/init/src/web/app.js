'use strict';
const byId = id => document.getElementById(id);
const size = bytes => bytes == null ? '—' : `${(bytes / 1048576).toFixed(1)} MiB`;
const states = {active: '运行中', pending: '启动中', aborted: '已中止', exec_retiring: '正在替换映像'};
let pending = false;
let timer;
let request;
async function refresh() {
  clearTimeout(timer);
  if (pending || document.hidden) return;
  pending = true;
  byId('refresh').disabled = true;
  request = new AbortController();
  const timeout = setTimeout(() => request?.abort(), 5000);
  try {
    const response = await fetch('/api/state', {cache: 'no-store', signal: request.signal});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const state = await response.json();
    byId('summary').textContent = `v${state.version} · ${state.platform} · init PID ${state.init_pid} · 已运行 ${Math.floor(state.uptime_ms / 1000)} 秒`;
    for (const key of ['processes', 'clients', 'objects', 'transactions']) byId(key).textContent = state.stats[key];
    const rows = document.createDocumentFragment();
    for (const process of state.processes) {
      const row = document.createElement('tr');
      const values = [`${process.identity.pid} / ${process.identity.parent_pid}`, process.identity.generation, process.host_pid,
        states[process.status] ?? process.status, size(process.native?.working_set_bytes), size(process.native?.private_bytes),
        process.native ? `${(process.native.cpu_time_ms / 1000).toFixed(2)} s` : '—'];
      for (const value of values) { const cell = document.createElement('td'); cell.textContent = value; row.append(cell); }
      rows.append(row);
    }
    byId('rows').replaceChildren(rows);
    byId('empty').hidden = state.processes.length !== 0;
    byId('connection').textContent = `${state.stopping ? '会话正在停止' : '已连接'} · 更新于 ${new Date().toLocaleTimeString()}`;
    byId('connection').className = '';
  } catch (error) {
    if (document.hidden) return;
    byId('connection').textContent = `连接中断，显示上次采样 · ${error.message}`;
    byId('connection').className = 'error';
  } finally {
    clearTimeout(timeout);
    request = null;
    pending = false;
    byId('refresh').disabled = false;
    if (!document.hidden) timer = setTimeout(refresh, 2000);
  }
}
byId('refresh').addEventListener('click', refresh);
document.addEventListener('visibilitychange', () => {
  clearTimeout(timer);
  if (document.hidden) request?.abort();
  else refresh();
});
refresh();
