import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import type { ConnectInfo, ServerMetrics, TransferTask } from '../shared/types.js';
import { connectWs, webApi } from './api-client.js';
import { HostTreeController } from './host-tree.js';
import { MonitorAndToolsController } from './monitor-panel.js';
import { SftpController } from './sftp-panel.js';

interface TabState {
  info: ConnectInfo;
  term: Terminal;
  fit: FitAddon;
  slot: HTMLDivElement;
  metrics: ServerMetrics | null;
  closed: boolean;
}

let activeId: string | null = null;
let fontSize = 14;
const tabs = new Map<string, TabState>();
const toastEl = document.getElementById('toast') as HTMLElement;

function showToast(msg: string): void {
  toastEl.textContent = msg;
  toastEl.classList.add('show');
  setTimeout(() => toastEl.classList.remove('show'), 2600);
}

function promptText(title: string, defaultVal = ''): Promise<string | null> {
  const dlg = document.getElementById('prompt-dialog') as HTMLDialogElement;
  const input = document.getElementById('prompt-input') as HTMLInputElement;
  const form = document.getElementById('prompt-form') as HTMLFormElement;
  (document.getElementById('prompt-title') as HTMLElement).textContent = title;
  input.value = defaultVal;
  return new Promise((resolve) => {
    const done = (val: string | null) => {
      dlg.close();
      form.onsubmit = null;
      resolve(val);
    };
    form.onsubmit = (e) => { e.preventDefault(); done(input.value.trim()); };
    (document.getElementById('cancel-prompt') as HTMLElement).onclick = () => done(null);
    dlg.showModal();
  });
}

const sftpCtrl = new SftpController({
  getActiveSshId: () => {
    const tab = activeId ? tabs.get(activeId) : null;
    return tab && !tab.closed && tab.info.kind === 'ssh' ? tab.info.sessionId : null;
  },
  toast: showToast,
  promptText,
});

const treeCtrl = new HostTreeController({
  onOpenHost: (hostId) => void openSshTab(hostId),
  toast: showToast,
  promptText,
});

const monCtrl = new MonitorAndToolsController({
  onSendCommand: (cmd) => sendInput(cmd),
  getHosts: () => treeCtrl.hosts,
  toast: showToast,
});

function handleWsEvent(msg: Record<string, unknown>): void {
  const type = String(msg.type || '');
  if (type === 'term:data' && typeof msg.sessionId === 'string' && typeof msg.data === 'string') {
    const tab = tabs.get(msg.sessionId);
    if (tab && !tab.closed) tab.term.write(Uint8Array.from(atob(msg.data), (c) => c.charCodeAt(0)));
  } else if ((type === 'term:closed' || type === 'session:closed') && typeof msg.sessionId === 'string') {
    markClosed(msg.sessionId);
  } else if (type === 'session:opened' && msg.session && typeof msg.session === 'object') {
    const info = msg.session as ConnectInfo;
    if (info.sessionId) createTermInstance(info);
  } else if (type === 'metrics:update' && typeof msg.sessionId === 'string') {
    const tab = tabs.get(msg.sessionId);
    if (!tab || tab.closed) return;
    tab.metrics = msg.metrics as ServerMetrics;
    if (activeId === msg.sessionId) showMetricsFor(tab);
  } else if (type === 'catalog:changed') {
    void refreshLive(String(msg.kind || 'disk'));
  } else if (type === 'transfer:progress') {
    sftpCtrl.onTransferProgress(msg.task as TransferTask);
  }
}

async function refreshLive(kind: string): Promise<void> {
  if (kind === 'snippets' || kind === 'disk') await monCtrl.refreshSnippets();
  if (kind === 'settings' || kind === 'disk') await applySettings();
  if (kind === 'tunnels' || kind === 'disk') monCtrl.refreshTunnelsIfOpen();
  if (kind === 'hosts' || kind === 'disk' || kind === '') await treeCtrl.refresh();
}

const ws = connectWs(handleWsEvent, () => { void refreshLive('disk'); });

function sendInput(data: string): void {
  if (!activeId || ws.readyState !== WebSocket.OPEN) return;
  const broadcast = (document.getElementById('broadcast-toggle') as HTMLInputElement).checked;
  ws.send(JSON.stringify(broadcast ? { type: 'term:broadcast', sessionIds: Array.from(tabs.keys()), data } : { type: 'term:input', sessionId: activeId, data }));
}

function createTermInstance(info: ConnectInfo): void {
  if (tabs.has(info.sessionId)) {
    selectTab(info.sessionId);
    return;
  }
  const stage = document.getElementById('terminal-stage') as HTMLElement;
  const slot = document.createElement('div');
  slot.className = 'term-slot hidden';
  stage.appendChild(slot);

  const term = new Terminal({
    fontFamily: '"Cascadia Code", "JetBrains Mono", Consolas, monospace',
    fontSize,
    cursorBlink: true,
    // 本地管道终端开启换行归位；远端 SSH PTY 保留原始控制流以免影响 TUI 应用
    convertEol: info.kind === 'local',
    scrollback: 5000,
    theme: { background: '#0b0e12', foreground: '#eae4d8', cursor: '#e3a008' },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(slot);
  term.onData((data) => sendInput(data));
  tabs.set(info.sessionId, { info, term, fit, slot, metrics: null, closed: false });
  selectTab(info.sessionId);
}

function showMetricsFor(tab: TabState | undefined): void {
  if (!tab || tab.closed || tab.info.kind !== 'ssh') {
    monCtrl.renderMetrics(null, 'idle');
    return;
  }
  monCtrl.renderMetrics(tab.metrics, tab.metrics ? 'live' : 'sampling');
}

function markClosed(sessionId: string): void {
  const tab = tabs.get(sessionId);
  if (!tab || tab.closed) return;
  tab.closed = true;
  tab.term.writeln('\r\n\x1b[33m[会话已断开]\x1b[0m');
  if (activeId === sessionId) {
    showMetricsFor(tab);
    sftpCtrl.syncSession();
  }
  renderTabs();
}

function selectTab(sessionId: string | null): void {
  activeId = sessionId;
  document.getElementById('term-empty')?.classList.toggle('hidden', tabs.size > 0);
  for (const [id, tab] of tabs) {
    const isAct = id === activeId;
    tab.slot.classList.toggle('hidden', !isAct);
    if (isAct) {
      tab.fit.fit();
      if (!tab.closed && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'term:resize', sessionId: id, cols: tab.term.cols, rows: tab.term.rows }));
      tab.term.focus();
      showMetricsFor(tab);
    }
  }
  if (!activeId) monCtrl.renderMetrics(null, 'idle');
  renderTabs();
  sftpCtrl.syncSession();
}

function renderTabs(): void {
  const bar = document.getElementById('tabs') as HTMLElement;
  bar.textContent = '';
  for (const [id, tab] of tabs) {
    const chip = document.createElement('div');
    chip.className = `tab-chip${id === activeId ? ' active' : ''}${tab.closed ? ' closed' : ''}`;
    const dot = document.createElement('span');
    dot.className = 'tab-dot';
    const label = document.createElement('span');
    label.textContent = tab.info.kind === 'local' ? '本机' : tab.info.alias;
    const closeBtn = document.createElement('button');
    closeBtn.type = 'button';
    closeBtn.textContent = '×';
    closeBtn.style.padding = '0 4px';
    closeBtn.addEventListener('click', (e) => { e.stopPropagation(); void closeTab(id); });
    chip.append(dot, label, closeBtn);
    chip.addEventListener('click', () => selectTab(id));
    bar.appendChild(chip);
  }
}

async function closeTab(sessionId: string): Promise<void> {
  const tab = tabs.get(sessionId);
  if (!tab) return;
  await webApi.closeSession(sessionId);
  tab.term.dispose();
  tab.slot.remove();
  tabs.delete(sessionId);
  selectTab(Array.from(tabs.keys()).pop() ?? null);
}

async function openSshTab(hostId: string): Promise<void> {
  try {
    createTermInstance(await webApi.openSession({ kind: 'ssh', hostId, cols: 100, rows: 30 }));
  } catch (err) {
    showToast(err instanceof Error ? err.message : '连接失败');
  }
}

function findInActiveTerm(needle: string, forward = false): void {
  if (!activeId || !needle) return;
  const tab = tabs.get(activeId);
  if (!tab) return;
  const buf = tab.term.buffer.active;
  const start = forward ? 0 : buf.length - 1;
  const end = forward ? buf.length : -1;
  const step = forward ? 1 : -1;
  for (let row = start; row !== end; row += step) {
    const col = (buf.getLine(row)?.translateToString() ?? '').toLowerCase().indexOf(needle.toLowerCase());
    if (col !== -1) {
      tab.term.scrollToLine(Math.max(0, row - 3));
      tab.term.select(col, row, needle.length);
      return;
    }
  }
  showToast(`未找到 "${needle}"`);
}

function bindGlobalUi(): void {
  document.getElementById('open-local')?.addEventListener('click', async () => createTermInstance(await webApi.openSession({ kind: 'local' })));
  document.getElementById('open-tunnels')?.addEventListener('click', () => void monCtrl.openTunnelModal());
  document.getElementById('inspector-toggle')?.addEventListener('click', () => document.getElementById('inspector')?.classList.toggle('hidden'));
  document.getElementById('theme-toggle')?.addEventListener('click', async () => {
    const next = document.documentElement.getAttribute('data-theme') === 'light' ? 'dark' : 'light';
    document.documentElement.setAttribute('data-theme', next);
    await webApi.updateSettings({ theme: next });
  });
  document.getElementById('search-toggle')?.addEventListener('click', () => document.getElementById('search-bar')?.classList.toggle('hidden'));
  document.getElementById('search-close')?.addEventListener('click', () => document.getElementById('search-bar')?.classList.add('hidden'));
  document.getElementById('search-prev')?.addEventListener('click', () => findInActiveTerm((document.getElementById('search-input') as HTMLInputElement).value, false));
  document.getElementById('search-next')?.addEventListener('click', () => findInActiveTerm((document.getElementById('search-input') as HTMLInputElement).value, true));

  // Split pane vertical drag resizer
  const handle = document.getElementById('split-handle');
  const termPane = document.getElementById('terminal-pane');
  const sftpPane = document.getElementById('sftp-pane');
  const wrap = document.getElementById('split-wrap');
  handle?.addEventListener('mousedown', (down) => {
    if (!termPane || !sftpPane || !wrap) return;
    down.preventDefault();
    const startY = down.clientY;
    const startH = termPane.getBoundingClientRect().height;
    const onMove = (mv: MouseEvent) => {
      const nextH = Math.max(120, Math.min(wrap.getBoundingClientRect().height - 120, startH + (mv.clientY - startY)));
      termPane.style.flex = `0 0 ${nextH}px`;
      if (activeId) tabs.get(activeId)?.fit.fit();
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  });

  const setDlg = document.getElementById('settings-dialog') as HTMLDialogElement;
  document.getElementById('open-settings')?.addEventListener('click', () => setDlg.showModal());
  document.getElementById('cancel-settings')?.addEventListener('click', () => setDlg.close());
  document.getElementById('settings-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const s = await webApi.updateSettings({
      theme: (document.getElementById('set-theme') as HTMLSelectElement).value as 'dark' | 'light',
      fontSize: Number((document.getElementById('set-font') as HTMLInputElement).value),
      monitorIntervalMs: Number((document.getElementById('set-interval') as HTMLInputElement).value),
      showHiddenFiles: (document.getElementById('set-hidden') as HTMLInputElement).checked,
    });
    fontSize = s.fontSize;
    document.documentElement.setAttribute('data-theme', s.theme);
    sftpCtrl.setShowHidden(s.showHiddenFiles);
    for (const tab of tabs.values()) { tab.term.options.fontSize = fontSize; tab.fit.fit(); }
    setDlg.close();
  });
}

window.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f' && activeId) {
    e.preventDefault();
    document.getElementById('search-bar')?.classList.remove('hidden');
    (document.getElementById('search-input') as HTMLInputElement).focus();
  }
});

window.addEventListener('resize', () => {
  const tab = activeId ? tabs.get(activeId) : null;
  if (tab) {
    tab.fit.fit();
    if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'term:resize', sessionId: activeId, cols: tab.term.cols, rows: tab.term.rows }));
  }
});

async function applySettings(): Promise<void> {
  const s = await webApi.getSettings();
  fontSize = s.fontSize;
  document.documentElement.setAttribute('data-theme', s.theme);
  sftpCtrl.setShowHidden(s.showHiddenFiles);
  const theme = document.getElementById('set-theme') as HTMLSelectElement | null;
  const font = document.getElementById('set-font') as HTMLInputElement | null;
  const interval = document.getElementById('set-interval') as HTMLInputElement | null;
  const hidden = document.getElementById('set-hidden') as HTMLInputElement | null;
  if (theme) theme.value = s.theme;
  if (font) font.value = String(s.fontSize);
  if (interval) interval.value = String(s.monitorIntervalMs);
  if (hidden) hidden.checked = s.showHiddenFiles;
  for (const tab of tabs.values()) {
    tab.term.options.fontSize = fontSize;
    tab.fit.fit();
  }
}

async function boot(): Promise<void> {
  bindGlobalUi();
  await refreshLive('disk');
}

void boot();
