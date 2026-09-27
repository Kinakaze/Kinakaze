'use strict';
const $ = id => document.getElementById(id);
const all = selector => [...document.querySelectorAll(selector)];
const text = (id, value) => { $(id).textContent = value; };
const node = (tag, className, value) => {
  const result = document.createElement(tag);
  if (className) result.className = className;
  if (value != null) result.textContent = value;
  return result;
};
const labels = {active: '运行中', pending: '启动中', aborted: '已中止', exec_retiring: '切换中'};
const status = process => process.standby ? '待命' : labels[process.status] ?? '未知';
const name = process => process.program?.split('/').filter(Boolean).at(-1) || (process.standby ? '待命运行环境' : 'Linux 程序');
const bytes = value => value == null ? '—' : value >= 1073741824 ? (value / 1073741824).toFixed(2) + ' GiB' : (value / 1048576).toFixed(1) + ' MiB';
const percent = value => value == null ? '—' : value.toFixed(1) + '%';
const clock = value => new Date(value).toLocaleTimeString('zh-CN', {hour12: false});
const duration = milliseconds => {
  const minutes = Math.floor(milliseconds / 60000);
  return minutes >= 60 ? Math.floor(minutes / 60) + ' 小时 ' + minutes % 60 + ' 分钟' : minutes > 0 ? minutes + ' 分钟' : '不足 1 分钟';
};
function preference(key, fallback) {
  try { return localStorage.getItem('kinakaze.' + key) ?? fallback; } catch { return fallback; }
}
function save(key, value) {
  try { localStorage.setItem('kinakaze.' + key, value); } catch { /* Storage may be disabled. */ }
}
let state = null;
let previous = null;
let online = false;
let paused = false;
let fetching = false;
let request = null;
let timer;
let toastTimer;
let busy = false;
let failures = 0;
let lastUpdate = null;
let filter = 'all';
let sorting = {key: null, descending: true};
let selected = null;
let termination = null;
let cpu = new Map();
const activities = [];
const launched = new Map();
let interval = Number(preference('interval', '2000'));
if (![2000, 5000, 10000].includes(interval)) interval = 2000;
$('interval').value = String(interval);

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  text('theme', theme === 'dark' ? '浅色' : '深色');
  $('theme').setAttribute('aria-label', theme === 'dark' ? '切换浅色外观' : '切换深色外观');
}
setTheme(preference('theme', matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'));
$('theme').addEventListener('click', () => {
  const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
  setTheme(theme);
  save('theme', theme);
});

function toast(message) {
  text('toast', message);
  $('toast').hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { $('toast').hidden = true; }, 6000);
}
function activity(title, description) {
  activities.unshift({title, description, time: Date.now()});
  activities.splice(20);
  $('activity').replaceChildren(...activities.map(event => {
    const item = node('li');
    const content = node('div', 'event-text', event.title);
    content.append(node('small', '', event.description));
    const time = node('time', '', clock(event.time));
    time.dateTime = new Date(event.time).toISOString();
    item.append(time, content);
    return item;
  }));
}
function available() { return online && state && !state.stopping; }
function launchable() { return available() && (state.desktop ? state.desktop.ready && !state.desktop.failed && !state.desktop.closing : state.pool.enabled && !state.pool.failed); }
function controls() {
  for (const id of ['open-launch', 'try-example']) $(id).disabled = !launchable() || busy;
  $('launch-submit').disabled = !launchable() || busy || (!state?.desktop && state.pool.ready === 0);
  if (!busy) text('launch-submit', state?.pool?.enabled && state.pool.ready === 0 ? '正在准备运行环境…' : '启动程序');
  $('refresh').disabled = fetching;
  text('pause', paused ? '继续' : '暂停');
  $('pause').setAttribute('aria-pressed', String(paused));
}
function connection() {
  const label = !online ? (failures ? '连接中断' : '正在连接') : state.stopping ? '会话正在停止' : paused ? '采样已暂停' : '已连接';
  text('connection-text', label);
  $('connection').className = 'connection' + (!online && failures ? ' error' : paused || state?.stopping ? ' paused' : '');
  text('updated', lastUpdate ? (online ? '更新于 ' : '上次更新 ') + clock(lastUpdate) + (paused ? ' · 已暂停' : '') : '尚未更新');
  controls();
}
function metric(id, formatted) {
  const [value, unit] = formatted.split(' ');
  $(id).replaceChildren(document.createTextNode(value));
  if (unit) $(id).append(node('span', 'unit', unit));
}
function sample(next) {
  cpu = new Map();
  const elapsed = previous && next.session_id === previous.session_id ? next.uptime_ms - previous.uptime_ms : 0;
  if (elapsed > 0 && elapsed < Math.max(15000, interval * 3)) {
    const old = new Map(previous.processes.map(process => [process.instance, process]));
    for (const process of next.processes) {
      const before = old.get(process.instance)?.native?.cpu_time_ms;
      const now = process.native?.cpu_time_ms;
      if (before != null && now != null && now >= before) cpu.set(process.instance, (now - before) / elapsed * 100);
    }
  }
  previous = next;
}
function renderSummary() {
  const applications = state.processes.filter(process => !process.standby);
  const running = applications.filter(process => process.status === 'active');
  const measured = applications.filter(process => process.native);
  text('running', running.length);
  text('running-hint', applications.length ? '含子进程，共 ' + applications.length + ' 个' : '暂无应用进程');
  metric('memory', applications.length === 0 ? '0 MiB' : measured.length ? bytes(measured.reduce((sum, process) => sum + process.native.working_set_bytes, 0)) : '—');
  text('memory-hint', measured.length === applications.length ? '应用进程合计' : measured.length + ' / ' + applications.length + ' 个采样可用');
  const measuredCpu = applications.filter(process => cpu.has(process.instance));
  metric('cpu', applications.length === 0 ? '0 %' : measuredCpu.length === applications.length ? (measuredCpu.reduce((sum, process) => sum + cpu.get(process.instance), 0)).toFixed(1) + ' %' : '—');
  text('cpu-hint', applications.length && measuredCpu.length !== applications.length ? '等待连续采样' : '单核为 100%');
  text('pool-status', state.stopping || state.desktop?.closing ? '正在停止' : state.desktop ? (state.desktop.ready && !state.desktop.failed ? '已就绪' : '连接中') : !state.pool.enabled ? '监控模式' : state.pool.failed ? '准备失败' : state.pool.ready ? '已就绪' : '准备中');
  text('pool-hint', state.desktop ? state.desktop.terminals.length + ' 个持久终端 · 关闭窗口可再次连接' : !state.pool.enabled ? '启动程序需要独立会话' : state.pool.failed ? '请在启动终端检查错误' : state.pool.ready + ' / ' + state.pool.capacity + ' 个环境待命');
  text('process-count', state.processes.length);
  text('art-version', 'v' + state.version);
  text('footer-version', 'Kinakaze v' + state.version);
  text('session-info', '本地会话 · 已运行 ' + duration(state.uptime_ms) + ' · init ' + state.init_pid);
}

const rowCache = new Map();
function createRow(process) {
  const row = node('tr');
  row.dataset.instance = process.instance;
  for (let i = 0; i < 6; i++) row.append(node('td'));
  const identity = node('div', 'process-name');
  const title = node('div');
  const button = node('button', 'process-link');
  button.type = 'button';
  button.dataset.action = 'detail';
  title.append(button, node('small', 'process-meta'));
  identity.append(title);
  row.cells[0].append(identity);
  row.cells[1].append(node('span', 'badge'));
  const end = node('button', 'row-action', '结束');
  end.type = 'button';
  end.dataset.action = 'terminate';
  row.cells[5].append(end);
  return row;
}
function renderRows() {
  if (!state) return;
  const query = $('search').value.trim().toLocaleLowerCase();
  let processes = state.processes.filter(process => {
    const matches = filter === 'all' || (filter === 'standby' ? process.standby : !process.standby && process.status === 'active');
    return matches && [name(process), process.program ?? '', process.identity.pid, process.host_pid].join(' ').toLocaleLowerCase().includes(query);
  });
  if (sorting.key) {
    const value = process => sorting.key === 'memory' ? process.native?.working_set_bytes : cpu.get(process.instance);
    processes = processes.slice().sort((a, b) => {
      const left = value(a), right = value(b);
      if (left == null || right == null) return left == null ? (right == null ? a.identity.pid - b.identity.pid : 1) : -1;
      return (left - right) * (sorting.descending ? -1 : 1) || a.identity.pid - b.identity.pid;
    });
  }
  const keys = new Set(processes.map(process => process.instance));
  for (const [key, row] of rowCache) {
    if (!keys.has(key)) {
      if (row.contains(document.activeElement)) $('search').focus({preventScroll: true});
      row.remove();
      rowCache.delete(key);
    }
  }
  processes.forEach((process, index) => {
    const row = rowCache.get(process.instance) ?? createRow(process);
    rowCache.set(process.instance, row);
    const button = row.querySelector('.process-link');
    button.textContent = name(process);
    button.title = process.program ?? '查看进程详情';
    row.querySelector('.process-meta').textContent = 'PID ' + process.identity.pid + (process.identity.parent_pid ? ' · 父进程 ' + process.identity.parent_pid : ' · 根进程');
    const badge = row.querySelector('.badge');
    badge.textContent = status(process);
    badge.className = 'badge ' + (process.standby ? 'standby' : process.status);
    row.cells[2].textContent = bytes(process.native?.working_set_bytes);
    row.cells[3].textContent = percent(cpu.get(process.instance));
    row.cells[4].textContent = process.host_pid;
    const end = row.querySelector('.row-action');
    end.hidden = !process.can_terminate;
    end.disabled = !available() || busy;
    end.setAttribute('aria-label', '结束 ' + name(process) + '，PID ' + process.identity.pid);
    // Keep unchanged nodes in place so keyboard focus survives periodic updates.
    if ($('rows').children[index] !== row) $('rows').insertBefore(row, $('rows').children[index] ?? null);
  });
  $('empty').hidden = processes.length !== 0;
  const filtered = query || filter !== 'all';
  text('empty-title', filtered ? '没有匹配的进程' : '还没有运行中的程序');
  text('empty-description', filtered ? '修改关键词或清除筛选。' : '使用右上角的“启动程序”添加进程。');
  $('empty-action').hidden = !filtered && !launchable();
  text('empty-action', filtered ? '清除筛选' : '启动第一个程序');
  $('empty-action').dataset.clear = filtered ? 'true' : 'false';
  text('list-summary', '显示 ' + processes.length + ' / ' + state.processes.length + ' 个进程 · — 表示采样不可用');
}
function renderDetails() {
  if (!$('detail-dialog').open || !selected) return;
  const process = state?.processes.find(item => item.instance === selected);
  if (!process) {
    text('detail-note', '该进程已退出或切换执行映像，请关闭详情并查看最新列表。');
    $('detail-terminate').disabled = true;
    return;
  }
  text('detail-title', name(process));
  text('detail-subtitle', process.program ?? '此进程未提供程序路径');
  const items = [
    ['Linux PID', process.identity.pid], ['父进程 PID', process.identity.parent_pid || '无（根进程）'],
    ['状态', status(process)], ['Windows PID', process.host_pid],
    ['内存工作集', bytes(process.native?.working_set_bytes)], ['私有提交内存', bytes(process.native?.private_bytes)],
    ['CPU 占用', percent(cpu.get(process.instance))], ['累计 CPU 时间', process.native ? (process.native.cpu_time_ms / 1000).toFixed(2) + ' 秒' : '—'],
    ['执行代次', process.identity.generation], ['模块 / 状态项', process.modules + ' / ' + process.states]
  ];
  $('detail-values').replaceChildren(...items.map(([key, value]) => {
    const pair = node('div'); pair.append(node('dt', '', key), node('dd', '', value)); return pair;
  }));
  text('detail-note', !online ? '连接已中断，以上为上次采样。恢复连接后才能操作。' : process.standby ? '这是用于启动新程序的待命运行环境，由 Kinakaze 自动管理。' : '内存来自 Windows 宿主。CPU 以单核为 100%，缺少连续采样时显示 —。');
  $('detail-terminate').hidden = !process.can_terminate;
  $('detail-terminate').disabled = !available() || busy;
}
function render() {
  if (state) { renderSummary(); renderRows(); renderDetails(); }
  connection();
}
function schedule() {
  clearTimeout(timer);
  if (!paused && !document.hidden) timer = setTimeout(refresh, Math.min(15000, interval * Math.max(1, 2 ** Math.min(failures, 3))));
}
async function refresh() {
  clearTimeout(timer);
  if (fetching || document.hidden) return;
  fetching = true;
  controls();
  const controller = new AbortController();
  request = controller;
  const timeout = setTimeout(() => controller.abort(), 5000);
  try {
    const response = await fetch('/api/state', {cache: 'no-store', signal: controller.signal});
    if (!response.ok) throw new Error('HTTP ' + response.status);
    const next = await response.json();
    if (next.schema_version !== 1 || !Array.isArray(next.processes) || !next.pool || typeof next.csrf_token !== 'string') throw new Error('会话版本不兼容，请重新加载页面');
    if (document.hidden) return;
    if (state && state.session_id !== next.session_id) {
      previous = null;
      launched.clear();
      activity('已连接到新会话', '旧会话的进程操作已失效。');
    }
    sample(next);
    state = next;
    online = true;
    failures = 0;
    lastUpdate = Date.now();
    $('notice').hidden = !next.stopping && !next.pool.failed;
    text('notice', next.stopping ? '会话正在停止。请等待退出，或在终端重新启动会话。' : '运行环境准备失败。已运行的程序仍可管理，请在启动终端查看错误信息。');
    for (const [pid, program] of launched) {
      if (!next.processes.some(process => process.identity.pid === pid)) {
        activity('进程已退出', program + ' · PID ' + pid);
        launched.delete(pid);
      }
    }
  } catch (error) {
    if (document.hidden) return;
    online = false;
    failures++;
    previous = null;
    $('notice').hidden = false;
    text('notice', (state ? '连接已中断，保留上次数据。' : '暂时无法连接本地会话。') + '请确认启动终端仍在运行；' + (paused ? '点击刷新可重试。' : '正在自动重连，也可点击刷新。') + (error.name === 'AbortError' ? '请求超时。' : '（' + error.message + '）'));
    if (!state) {
      text('empty-title', '暂时无法连接工作空间');
      text('empty-description', '确认 init 正在运行，并使用终端显示的 WebUI 地址。');
    }
  } finally {
    clearTimeout(timeout);
    fetching = false;
    request = null;
    render();
    schedule();
  }
}
async function action(path, body) {
  if (!available()) throw new Error('会话未连接，请刷新后再试。');
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 8000);
  try {
    let response;
    try {
      response = await fetch(path, {
        method: 'POST', headers: {'Content-Type': 'application/json', 'X-Kinakaze-Token': state.csrf_token},
        body: JSON.stringify(body), signal: controller.signal
      });
    } catch {
      throw new Error('未收到操作结果。请求可能已被接受，请先刷新进程列表确认，避免重复操作。');
    }
    if (!response.ok) {
      let message = '操作未完成（HTTP ' + response.status + '），请刷新后重试。';
      try { message = (await response.json()).error || message; } catch { /* Non-JSON HTTP guard response. */ }
      throw new Error(message);
    }
    try { return await response.json(); }
    catch { throw new Error('操作结果不完整。请先刷新列表确认，避免重复操作。'); }
  } finally { clearTimeout(timeout); }
}
function showError(id, error) { text(id, error.message); $(id).hidden = false; }
function setBusy(value) {
  busy = value;
  all('#launch-dialog button, #launch-dialog input, #launch-dialog textarea, #terminate-dialog button').forEach(element => { element.disabled = value; });
  controls();
  renderRows();
  renderDetails();
}
function addArgument(value = '', focus = true) {
  if ($('arguments').children.length >= 127) { toast('最多添加 127 个启动参数。'); return; }
  const row = node('div', 'argument-row');
  const input = node('input');
  input.value = value; input.maxLength = 4096; input.autocomplete = 'off'; input.spellcheck = false;
  input.setAttribute('aria-label', '启动参数');
  const remove = node('button', 'icon-button', '×');
  remove.type = 'button'; remove.setAttribute('aria-label', '删除此参数');
  remove.addEventListener('click', () => { row.remove(); $('add-argument').focus(); });
  row.append(input, remove); $('arguments').append(row);
  if (focus) input.focus();
}
function openLaunch(example = false) {
  if (!launchable() || busy) return;
  if (example) {
    $('program').value = '/bin/sleep';
    $('cwd').value = '/';
    $('environment').value = '';
    $('arguments').replaceChildren();
    addArgument('60', false);
  }
  $('launch-error').hidden = true;
  controls();
  $('launch-dialog').showModal();
  $('program').focus();
}
function askTerminate(process) {
  if (!available() || !process.can_terminate || busy) return;
  termination = {pid: process.identity.pid, instance: process.instance, program: name(process)};
  text('terminate-target', name(process) + ' · PID ' + process.identity.pid);
  $('terminate-error').hidden = true;
  $('terminate-dialog').showModal();
  $('cancel-terminate').focus();
}
$('open-launch').addEventListener('click', () => openLaunch());
$('try-example').addEventListener('click', () => openLaunch(true));
$('add-argument').addEventListener('click', () => addArgument());
$('launch-form').addEventListener('invalid', event => {
  const details = event.target.closest('details');
  if (details) details.open = true;
}, true);
$('launch-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (busy || !launchable()) return;
  const environment = $('environment').value.split(/\r?\n/).filter(line => line.trim().length);
  if (environment.some(line => !/^[A-Za-z_][A-Za-z0-9_]*=/.test(line))) {
    showError('launch-error', new Error('环境变量请按 NAME=VALUE 填写，每行一项。'));
    document.querySelector('#launch-dialog .advanced').open = true;
    $('environment').focus(); return;
  }
  const body = {arguments: [$('program').value, ...all('#arguments input').map(input => input.value)], cwd: $('cwd').value, environment: environment.length ? environment : null};
  if (new TextEncoder().encode(JSON.stringify(body)).length > 16384) {
    showError('launch-error', new Error('启动参数过长，请减少参数或环境变量（总计最多 16 KiB）。')); return;
  }
  $('launch-error').hidden = true;
  setBusy(true); text('launch-submit', '正在提交…');
  try {
    const result = await action('/api/launch', body);
    launched.set(result.pid, body.arguments[0]);
    activity('启动请求已接受', body.arguments[0] + ' · PID ' + result.pid);
    $('launch-dialog').close();
    $('environment').value = '';
    toast('已提交启动请求 · PID ' + result.pid);
  } catch (error) { showError('launch-error', error); }
  finally { setBusy(false); refresh(); }
});
$('confirm-terminate').addEventListener('click', async () => {
  if (busy || !termination) return;
  setBusy(true); text('confirm-terminate', '正在结束…'); $('terminate-error').hidden = true;
  try {
    await action('/api/terminate', {pid: termination.pid, instance: termination.instance});
    activity('已请求结束进程', termination.program + ' · PID ' + termination.pid);
    $('terminate-dialog').close();
    if ($('detail-dialog').open) $('detail-dialog').close();
    toast('结束请求已发送，正在更新列表');
  } catch (error) { showError('terminate-error', error); }
  finally { setBusy(false); text('confirm-terminate', '确认结束'); refresh(); }
});
all('.close-dialog').forEach(button => button.addEventListener('click', () => { if (!busy) button.closest('dialog').close(); }));
all('dialog').forEach(dialog => {
  dialog.addEventListener('cancel', event => { if (busy) event.preventDefault(); });
});
$('rows').addEventListener('click', event => {
  const button = event.target.closest('button[data-action]');
  if (!button) return;
  const process = state?.processes.find(item => item.instance === button.closest('tr').dataset.instance);
  if (!process) return;
  if (button.dataset.action === 'terminate') askTerminate(process);
  else { selected = process.instance; $('detail-dialog').showModal(); renderDetails(); }
});
$('detail-terminate').addEventListener('click', () => {
  const process = state?.processes.find(item => item.instance === selected);
  if (process) askTerminate(process);
});
function setFilter(value) {
  filter = value;
  all('[data-filter]').forEach(button => {
    button.classList.toggle('active', button.dataset.filter === value);
    button.setAttribute('aria-pressed', String(button.dataset.filter === value));
  });
  renderRows();
}
all('[data-filter]').forEach(button => button.addEventListener('click', () => setFilter(button.dataset.filter)));
$('search').addEventListener('input', renderRows);
$('empty-action').addEventListener('click', () => {
  if ($('empty-action').dataset.clear === 'true') { $('search').value = ''; setFilter('all'); $('search').focus(); }
  else openLaunch();
});
all('[data-sort]').forEach(button => button.addEventListener('click', () => {
  sorting = {key: button.dataset.sort, descending: sorting.key === button.dataset.sort ? !sorting.descending : true};
  all('[data-sort]').forEach(item => {
    const active = item.dataset.sort === sorting.key;
    item.closest('th').setAttribute('aria-sort', active ? sorting.descending ? 'descending' : 'ascending' : 'none');
    item.querySelector('span').textContent = active ? sorting.descending ? '↓' : '↑' : '↕';
  });
  renderRows();
}));
$('refresh').addEventListener('click', refresh);
$('pause').addEventListener('click', () => {
  paused = !paused; clearTimeout(timer); previous = null;
  connection();
  if (!paused) refresh();
});
$('interval').addEventListener('change', () => {
  interval = Number($('interval').value); save('interval', String(interval)); schedule();
});
document.addEventListener('visibilitychange', () => {
  clearTimeout(timer); previous = null;
  if (document.hidden) request?.abort();
  else if (!paused) refresh();
});
$('help').addEventListener('click', () => $('help-dialog').showModal());
function setArt(visible) {
  document.documentElement.dataset.art = visible ? 'on' : 'off';
  $('art-toggle').setAttribute('aria-pressed', String(visible));
  $('art-toggle').setAttribute('aria-label', visible ? '收起角色插画' : '显示角色插画');
}
setArt(preference('art', 'on') !== 'off');
$('art-toggle').addEventListener('click', () => {
  const visible = document.documentElement.dataset.art !== 'on';
  setArt(visible);
  save('art', visible ? 'on' : 'off');
});
refresh();
