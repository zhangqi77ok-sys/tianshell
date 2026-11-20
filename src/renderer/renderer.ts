import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import type { ConnectInfo, ServerMetrics, TransferTask } from '../shared/types.js';
import { connectWs, resolveClientToken, webApi } from './api-client.js';
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

const TERM_FONT = 'Consolas, "Cascadia Mono", "Courier New", monospace';
const TERM_THEME = {
  background: '#0c0c0c',
  foreground: '#cccccc',
  cursor: '#f2f2f2',
  cursorAccent: '#0c0c0c',
  selectionBackground: '#264f78',
  black: '#0c0c0c',
  red: '#c50f1f',
  green: '#13a10e',
  yellow: '#c19c00',
  blue: '#3b78ff',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#cccccc',
  brightBlack: '#767676',
  brightRed: '#e74856',
  brightGreen: '#16c60c',
  brightYellow: '#f9f1a5',
  brightBlue: '#3b78ff',
  brightMagenta: '#b4009e',
  brightCyan: '#61d6d6',
  brightWhite: '#f2f2f2',
};

type SplitMode = 'single' | 'horizontal' | 'vertical';
let currentSplitMode: SplitMode = 'single';
let secondaryId: string | null = null;
let activeId: string | null = null;
let fontSize = 14;
const tabs = new Map<string, TabState>();
const pendingConnections = new Map<string, { alias: string; host?: string; port?: number }>();
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
  getActiveHostIds: () => new Set(Array.from(tabs.values()).map(t => t.info.hostId).filter(Boolean) as string[]),
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
  if (kind === 'history' || kind === 'disk') await monCtrl.refreshHistory();
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
  slot.addEventListener('pointerdown', () => {
    if (activeId !== info.sessionId) {
      selectTab(info.sessionId);
    }
  });
  stage.appendChild(slot);

  const term = new Terminal({
    fontFamily: TERM_FONT,
    fontSize,
    fontWeight: 400,
    fontWeightBold: 600,
    lineHeight: 1.15,
    letterSpacing: 0,
    cursorBlink: true,
    cursorStyle: 'bar',
    cursorWidth: 2,
    // 本地管道终端开启换行归位；远端 SSH PTY 自己发 CR，不能再转换以免打乱网格
    convertEol: info.kind === 'local',
    scrollback: 8000,
    theme: TERM_THEME,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(slot);
  term.onData((data) => sendInput(data));
  tabs.set(info.sessionId, { info, term, fit, slot, metrics: null, closed: false });
  treeCtrl.render();
  selectTab(info.sessionId);
}

function fitTab(tab: TabState): void {
  if (tab.slot.classList.contains('hidden')) return;
  tab.fit.fit();
  const { cols, rows } = tab.term;
  if (!tab.closed && cols > 1 && rows > 1 && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: 'term:resize', sessionId: tab.info.sessionId, cols, rows }));
  }
}

function measureGrid(): { cols: number; rows: number } {
  const stage = document.getElementById('terminal-stage');
  if (!stage) return { cols: 100, rows: 28 };
  const slot = document.createElement('div');
  slot.className = 'term-slot';
  stage.appendChild(slot);
  const term = new Terminal({
    fontFamily: TERM_FONT,
    fontSize,
    lineHeight: 1.15,
    letterSpacing: 0,
    theme: TERM_THEME,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(slot);
  fit.fit();
  const cols = term.cols > 1 ? term.cols : 100;
  const rows = term.rows > 1 ? term.rows : 28;
  term.dispose();
  slot.remove();
  return { cols, rows };
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

function setSplitMode(mode: SplitMode): void {
  if (mode !== 'single' && tabs.size < 2) {
    showToast('分屏需要至少打开 2 个终端会话');
    return;
  }
  currentSplitMode = mode;
  updateSplitControls();
  updateSplitLayout();
}

function updateSplitControls(): void {
  document.getElementById('split-single')?.classList.toggle('active', currentSplitMode === 'single');
  document.getElementById('split-horizontal')?.classList.toggle('active', currentSplitMode === 'horizontal');
  document.getElementById('split-vertical')?.classList.toggle('active', currentSplitMode === 'vertical');
}

function fitVisibleTabs(): void {
  if (activeId) {
    const tab = tabs.get(activeId);
    if (tab) fitTab(tab);
  }
  if (currentSplitMode !== 'single' && secondaryId && secondaryId !== activeId) {
    const secTab = tabs.get(secondaryId);
    if (secTab) fitTab(secTab);
  }
}

function updateSplitLayout(): void {
  const stageEl = document.getElementById('terminal-stage');
  if (!stageEl) return;

  if (currentSplitMode !== 'single' && tabs.size < 2) {
    currentSplitMode = 'single';
    updateSplitControls();
  }

  if (currentSplitMode !== 'single') {
    if (!secondaryId || !tabs.has(secondaryId) || secondaryId === activeId) {
      const other = Array.from(tabs.keys()).find((id) => id !== activeId);
      secondaryId = other ?? null;
    }
  } else {
    secondaryId = null;
  }

  if (currentSplitMode !== 'single' && !secondaryId) {
    currentSplitMode = 'single';
    updateSplitControls();
  }

  stageEl.classList.toggle('split-h', currentSplitMode === 'horizontal');
  stageEl.classList.toggle('split-v', currentSplitMode === 'vertical');

  for (const [id, tab] of tabs) {
    const isPrimary = id === activeId;
    const isSecondary = currentSplitMode !== 'single' && id === secondaryId;
    const isVisible = isPrimary || isSecondary;

    tab.slot.classList.toggle('hidden', !isVisible);
    tab.slot.classList.toggle('active-pane', isPrimary);

    if (isVisible) {
      fitTab(tab);
      if (isPrimary) {
        tab.term.focus();
        showMetricsFor(tab);
      }
    }
  }

  renderTabs();
  sftpCtrl.syncSession();
}

function selectTab(sessionId: string | null): void {
  if (!sessionId) {
    activeId = null;
    secondaryId = null;
    currentSplitMode = 'single';
    updateSplitControls();
    document.getElementById('term-empty')?.classList.remove('hidden');
    monCtrl.renderMetrics(null, 'idle');
    renderTabs();
    sftpCtrl.syncSession();
    return;
  }

  document.getElementById('term-empty')?.classList.add('hidden');

  if (currentSplitMode !== 'single') {
    if (sessionId === secondaryId) {
      const prevActive = activeId;
      activeId = sessionId;
      secondaryId = prevActive;
    } else {
      activeId = sessionId;
      if (secondaryId === activeId) {
        const other = Array.from(tabs.keys()).find((id) => id !== activeId);
        secondaryId = other ?? null;
      }
    }
    updateSplitLayout();
  } else {
    activeId = sessionId;
    secondaryId = null;
    for (const [id, tab] of tabs) {
      const isAct = id === activeId;
      tab.slot.classList.toggle('hidden', !isAct);
      tab.slot.classList.toggle('active-pane', isAct);
      if (isAct) {
        fitTab(tab);
        tab.term.focus();
        showMetricsFor(tab);
      }
    }
    renderTabs();
    sftpCtrl.syncSession();
  }
}

function renderTabs(): void {
  const bar = document.getElementById('tabs') as HTMLElement;
  bar.textContent = '';
  for (const [id, tab] of tabs) {
    const isPrimary = id === activeId;
    const isSecondary = currentSplitMode !== 'single' && id === secondaryId;
    const chip = document.createElement('div');
    chip.className = `tab-chip${isPrimary ? ' active' : ''}${tab.closed ? ' closed' : ''}`;
    const dot = document.createElement('span');
    dot.className = 'tab-dot';
    const label = document.createElement('span');
    label.textContent = tab.info.kind === 'local' ? '本机' : tab.info.alias;
    chip.append(dot, label);

    if (currentSplitMode !== 'single') {
      if (isPrimary) {
        const badge = document.createElement('span');
        badge.className = 'pane-badge primary';
        badge.textContent = '主屏';
        chip.appendChild(badge);
      } else if (isSecondary) {
        const badge = document.createElement('span');
        badge.className = 'pane-badge secondary';
        badge.textContent = '副屏';
        chip.appendChild(badge);
      }
    }

    const closeBtn = document.createElement('button');
    closeBtn.type = 'button';
    closeBtn.textContent = '×';
    closeBtn.style.padding = '0 4px';
    closeBtn.addEventListener('click', (e) => { e.stopPropagation(); void closeTab(id); });
    chip.appendChild(closeBtn);
    chip.addEventListener('click', () => selectTab(id));
    bar.appendChild(chip);
  }
  for (const [, p] of pendingConnections) {
    const chip = document.createElement('div');
    chip.className = 'tab-chip connecting active';
    const dot = document.createElement('span');
    dot.className = 'tab-dot spin';
    const label = document.createElement('span');
    label.textContent = `${p.alias} (连接中...)`;
    chip.append(dot, label);
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
  treeCtrl.render();

  if (secondaryId === sessionId) {
    secondaryId = null;
  }
  if (tabs.size < 2 && currentSplitMode !== 'single') {
    currentSplitMode = 'single';
    updateSplitControls();
    document.getElementById('terminal-stage')?.classList.remove('split-h', 'split-v');
  }

  if (activeId === sessionId) {
    selectTab(Array.from(tabs.keys()).pop() ?? null);
  } else {
    updateSplitLayout();
  }
}

async function openSshTab(hostId: string): Promise<void> {
  if (pendingConnections.has(hostId)) return;
  const host = treeCtrl.hosts.find((h) => h.id === hostId);
  const alias = host ? host.alias : '远程主机';
  pendingConnections.set(hostId, { alias, host: host?.host, port: host?.port });

  const hostEl = document.getElementById(`host-${hostId}`);
  hostEl?.classList.add('connecting');

  const termEmpty = document.getElementById('term-empty');
  if (termEmpty && tabs.size === 0) {
    termEmpty.classList.remove('hidden');
    termEmpty.innerHTML = `
      <div class="connecting-box">
        <div class="connecting-spinner"></div>
        <div class="connecting-title">正在连接 ${alias}...</div>
        <div class="connecting-sub">${host ? `${host.username}@${host.host}:${host.port}` : ''} · 正在协商密钥并分配终端</div>
      </div>
    `;
  }
  renderTabs();

  try {
    const grid = measureGrid();
    const info = await webApi.openSession({ kind: 'ssh', hostId, cols: grid.cols, rows: grid.rows });
    if (!info.hostId) info.hostId = hostId;
    pendingConnections.delete(hostId);
    hostEl?.classList.remove('connecting');
    if (termEmpty) {
      termEmpty.innerHTML = '<div>没有打开的终端</div><small>双击左侧主机，或使用「本机终端」。</small>';
    }
    createTermInstance(info);
    const tab = activeId ? tabs.get(activeId) : undefined;
    if (tab) requestAnimationFrame(() => fitTab(tab));
  } catch (err) {
    pendingConnections.delete(hostId);
    hostEl?.classList.remove('connecting');
    if (termEmpty) {
      termEmpty.innerHTML = '<div>没有打开的终端</div><small>双击左侧主机，或使用「本机终端」。</small>';
    }
    renderTabs();
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
  document.getElementById('open-local')?.addEventListener('click', async () => {
    const grid = measureGrid();
    createTermInstance(await webApi.openSession({ kind: 'local', cols: grid.cols, rows: grid.rows }));
  });
  document.getElementById('split-single')?.addEventListener('click', () => setSplitMode('single'));
  document.getElementById('split-horizontal')?.addEventListener('click', () => setSplitMode('horizontal'));
  document.getElementById('split-vertical')?.addEventListener('click', () => setSplitMode('vertical'));
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
      const nextH = Math.max(160, Math.min(wrap.getBoundingClientRect().height - 140, startH + (mv.clientY - startY)));
      termPane.style.flex = `0 0 ${nextH}px`;
      sftpPane.style.flex = '1 1 auto';
      sftpPane.style.maxHeight = 'none';
      fitVisibleTabs();
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
  fitVisibleTabs();
});
const stage = document.getElementById('terminal-stage');
if (stage && 'ResizeObserver' in window) {
  const observer = new ResizeObserver(() => {
    fitVisibleTabs();
  });
  observer.observe(stage);
}
void document.fonts?.ready.then(() => {
  fitVisibleTabs();
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

function mcpConfigs(url: string, token: string): { cursor: string; claude: string } {
  const cursor = {
    mcpServers: {
      tianshell: {
        url,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
  const claude = {
    mcpServers: {
      tianshell: {
        type: 'http',
        url,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
  return { cursor: JSON.stringify(cursor, null, 2), claude: JSON.stringify(claude, null, 2) };
}

async function copyText(text: string, ok: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    showToast(ok);
  } catch {
    showToast('复制失败，请在对话框里手动选择');
  }
}

async function showMcpDialog(): Promise<void> {
  const dlg = document.getElementById('mcp-dialog') as HTMLDialogElement;
  const info = await webApi.mcpInfo();
  const token = resolveClientToken();
  const configs = mcpConfigs(info.url, token);
  (document.getElementById('mcp-url') as HTMLInputElement).value = info.url;
  (document.getElementById('mcp-token') as HTMLInputElement).value = token;
  (document.getElementById('mcp-cursor') as HTMLTextAreaElement).value = configs.cursor;
  (document.getElementById('mcp-claude') as HTMLTextAreaElement).value = configs.claude;
  const status = document.getElementById('mcp-status');
  if (status) status.textContent = info.url.replace(/^https?:\/\//, '');
  dlg.showModal();
}

async function boot(): Promise<void> {
  bindGlobalUi();
  document.getElementById('open-mcp')?.addEventListener('click', () => void showMcpDialog().catch((err: unknown) => {
    showToast(err instanceof Error ? err.message : '读不到 MCP 地址');
  }));
  document.getElementById('close-mcp')?.addEventListener('click', () => {
    (document.getElementById('mcp-dialog') as HTMLDialogElement).close();
  });
  document.getElementById('copy-cursor')?.addEventListener('click', () => {
    void copyText((document.getElementById('mcp-cursor') as HTMLTextAreaElement).value, '已复制 Cursor 配置');
  });
  document.getElementById('copy-claude')?.addEventListener('click', () => {
    void copyText((document.getElementById('mcp-claude') as HTMLTextAreaElement).value, '已复制 Claude Code 配置');
  });
  try {
    const info = await webApi.mcpInfo();
    const status = document.getElementById('mcp-status');
    if (status) status.textContent = info.url.replace(/^https?:\/\//, '');
  } catch {
    const status = document.getElementById('mcp-status');
    if (status) status.textContent = 'MCP 地址暂时读不到';
  }
  await refreshLive('disk');
}

void boot();
