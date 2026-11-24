import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import type { ConnectInfo, ServerMetrics, TransferTask } from '../shared/types.js';
import { connectWs, resolveClientToken, webApi } from './api-client.js';
import { HostTreeController } from './host-tree.js';
import { MonitorAndToolsController } from './monitor-panel.js';
import {
  acceptLiveMetrics,
  broadcastRecipients,
  claimSingleFlight,
  clearScreenInput,
  quickCommandInput,
  inputDelivery,
  keystrokeRecipients,
  nextFindIndex,
  shouldNotifyLinkDown,
  planSessionOpen,
  resizeIfChanged,
  seedBroadcastChecks,
  sessionOpenSteps,
  pasteNeedsConfirm,
  settlePaste,
  shouldMeasureTerminal,
  tabLabels,
  type BroadcastTab,
  type SessionOpenIntent,
} from './session-logic.js';
import { SftpController } from './sftp-panel.js';
import { resolveSplitVisibility } from './sftp-split-logic.js';

interface TabState {
  info: ConnectInfo & { hostId?: string };
  term: Terminal;
  fit: FitAddon;
  slot: HTMLDivElement;
  metrics: ServerMetrics | null;
  closed: boolean;
  dataSub: { dispose(): void };
  lastResize: { cols: number; rows: number } | null;
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

const TERM_THEME_LIGHT = {
  background: '#f6f3ec',
  foreground: '#1c1915',
  cursor: '#1c1915',
  cursorAccent: '#f6f3ec',
  selectionBackground: '#c9daf8',
  black: '#1c1915',
  red: '#c2410c',
  green: '#3f6212',
  yellow: '#a16207',
  blue: '#1d4ed8',
  magenta: '#7e22ce',
  cyan: '#0f766e',
  white: '#44403c',
  brightBlack: '#78716c',
  brightRed: '#dc2626',
  brightGreen: '#15803d',
  brightYellow: '#ca8a04',
  brightBlue: '#2563eb',
  brightMagenta: '#a21caf',
  brightCyan: '#0e7490',
  brightWhite: '#1c1915',
};

function termTheme(): typeof TERM_THEME {
  return document.documentElement.getAttribute('data-theme') === 'light' ? TERM_THEME_LIGHT : TERM_THEME;
}

function applyTermTheme(): void {
  const theme = termTheme();
  for (const tab of tabs.values()) tab.term.options.theme = theme;
}

function syncThemeToggle(): void {
  const btn = document.getElementById('theme-toggle');
  if (!btn) return;
  const light = document.documentElement.getAttribute('data-theme') === 'light';
  btn.textContent = light ? '亮色' : '暗色';
  btn.title = light ? '当前是亮色，点击改成暗色' : '当前是暗色，点击改成亮色';
}

type SplitMode = 'single' | 'horizontal' | 'vertical';
type ViewMode = 'terminal' | 'sftp' | 'monitor';
let currentView: ViewMode = 'terminal';
let currentSplitMode: SplitMode = 'single';
let secondaryId: string | null = null;
let activeId: string | null = null;
let fontSize = 14;
const tabs = new Map<string, TabState>();
interface PendingConnect {
  alias: string;
  detail: string;
  abort: AbortController;
  cancelled: boolean;
}
const pendingConnections = new Map<string, PendingConnect>();
const cancelledHostIds = new Set<string>();
const hostErrors = new Map<string, string>();
const broadcastChecked = new Set<string>();
let selectedHostId: string | null = null;
const toastEl = document.getElementById('toast') as HTMLElement;
let toastTimer = 0;

function markSelectedHost(hostId: string | null): void {
  selectedHostId = hostId;
  document.querySelectorAll('.host-item').forEach((el) => {
    const selected = hostId !== null && el.id === `host-${hostId}`;
    el.classList.toggle('selected', selected);
    el.setAttribute('aria-selected', selected ? 'true' : 'false');
  });
}

function showToast(msg: string, kind: 'info' | 'error' = 'info'): void {
  window.clearTimeout(toastTimer);
  toastEl.textContent = msg;
  toastEl.classList.toggle('error', kind === 'error');
  toastEl.classList.add('show');
  toastTimer = window.setTimeout(() => toastEl.classList.remove('show'), kind === 'error' ? 4600 : 2200);
}

function confirmAction(title: string, body: string, okLabel = '删除'): Promise<boolean> {
  const dlg = document.getElementById('confirm-dialog') as HTMLDialogElement;
  const form = document.getElementById('confirm-form') as HTMLFormElement;
  const ok = document.getElementById('confirm-ok') as HTMLButtonElement;
  (document.getElementById('confirm-title') as HTMLElement).textContent = title;
  (document.getElementById('confirm-body') as HTMLElement).textContent = body;
  ok.textContent = okLabel;
  return new Promise((resolve) => {
    const onCancel = () => finish(false);
    const finish = (value: boolean) => {
      dlg.removeEventListener('cancel', onCancel);
      dlg.close();
      form.onsubmit = null;
      resolve(value);
    };
    form.onsubmit = (event) => {
      event.preventDefault();
      finish(true);
    };
    (document.getElementById('cancel-confirm') as HTMLElement).onclick = () => finish(false);
    dlg.addEventListener('cancel', onCancel);
    dlg.showModal();
  });
}

function promptText(title: string, defaultVal = ''): Promise<string | null> {
  const dlg = document.getElementById('prompt-dialog') as HTMLDialogElement;
  const input = document.getElementById('prompt-input') as HTMLInputElement;
  const form = document.getElementById('prompt-form') as HTMLFormElement;
  (document.getElementById('prompt-title') as HTMLElement).textContent = title;
  input.value = defaultVal;
  return new Promise((resolve) => {
    let settled = false;
    const finish = (val: string | null) => {
      if (settled) return;
      settled = true;
      dlg.removeEventListener('cancel', onCancel);
      form.onsubmit = null;
      dlg.close();
      resolve(val);
    };
    const onCancel = (event: Event) => {
      event.preventDefault();
      finish(null);
    };
    form.onsubmit = (event) => {
      event.preventDefault();
      finish(input.value.trim());
    };
    (document.getElementById('cancel-prompt') as HTMLElement).onclick = () => finish(null);
    dlg.addEventListener('cancel', onCancel);
    dlg.showModal();
  });
}

const sftpCtrl = new SftpController({
  getActiveSshId: () => {
    const tab = activeId ? tabs.get(activeId) : null;
    return tab && !tab.closed && tab.info.kind === 'ssh' ? tab.info.sessionId : null;
  },
  getActiveSessionInfo: () => {
    const tab = activeId ? tabs.get(activeId) : null;
    if (!tab) return null;
    return {
      sessionId: tab.info.sessionId,
      kind: tab.info.kind,
      alias: tab.info.alias,
      username: tab.info.username,
      host: tab.info.host,
      port: tab.info.port,
      closed: tab.closed,
    };
  },
  onSyncTerminalPwd: (dir: string) => {
    if (!activeId) {
      showToast('请先打开或选中一个终端会话', 'error');
      return;
    }
    const tab = tabs.get(activeId);
    if (!tab || tab.closed) {
      showToast('当前终端会话已断开', 'error');
      return;
    }
    sendQuickCommand(`cd "${dir}"\n`);
    showToast(`已在终端执行: cd "${dir}"`);
  },
  toast: showToast,
  promptText,
  confirm: confirmAction,
  onDirChange: () => updateSessionContext(),
});

const treeCtrl = new HostTreeController({
  onOpenHost: (hostId) => void openSshTab(hostId, true, 'reuse'),
  onOpenAnother: (hostId) => void openSshTab(hostId, true, 'another'),
  onSelectHost: (hostId) => markSelectedHost(hostId),
  onCancelConnect: (hostId) => cancelConnect(hostId),
  toast: showToast,
  promptText,
  confirm: confirmAction,
  getActiveHostIds: () => new Set(Array.from(tabs.values()).filter((t) => !t.closed).map((t) => t.info.hostId).filter(Boolean) as string[]),
  getHostUi: (hostId) => {
    const pending = pendingConnections.get(hostId);
    return {
      connecting: Boolean(pending) && !pending?.cancelled,
      cancelling: Boolean(pending?.cancelled),
      error: hostErrors.get(hostId) ?? null,
      selected: selectedHostId === hostId,
    };
  },
});

const monCtrl = new MonitorAndToolsController({
  onFillCommand: (cmd) => fillCommand(cmd),
  getHosts: () => treeCtrl.hosts,
  toast: showToast,
  confirm: confirmAction,
  reloadQuickOps: () => { void renderQuickOpsBar(); },
});

function handleWsEvent(msg: Record<string, unknown>): void {
  const type = String(msg.type || '');
  if (type === 'term:data' && typeof msg.sessionId === 'string' && typeof msg.data === 'string') {
    const tab = tabs.get(msg.sessionId);
    if (tab && !tab.closed) tab.term.write(Uint8Array.from(atob(msg.data), (c) => c.charCodeAt(0)));
  } else if ((type === 'term:closed' || type === 'session:closed') && typeof msg.sessionId === 'string') {
    markClosed(msg.sessionId);
  } else if (type === 'session:opened' && msg.session && typeof msg.session === 'object') {
    const info = msg.session as ConnectInfo & { hostId?: string };
    if (info.hostId && cancelledHostIds.has(info.hostId)) {
      cancelledHostIds.delete(info.hostId);
      pendingConnections.delete(info.hostId);
      void webApi.closeSession(info.sessionId);
      renderTabs();
      treeCtrl.render();
      if (tabs.size === 0) showIdleEmpty();
      return;
    }
    if (info.sessionId) createTermInstance(info);
  } else if (type === 'metrics:update' && typeof msg.sessionId === 'string') {
    const tab = tabs.get(msg.sessionId);
    if (!tab || tab.closed) return;
    tab.metrics = msg.metrics as ServerMetrics;
    if (acceptLiveMetrics(msg.sessionId, activeId, tab.closed)) showMetricsFor(tab);
  } else if (type === 'catalog:changed') {
    void refreshLive(String(msg.kind || 'disk'));
  } else if (type === 'transfer:progress') {
    sftpCtrl.onTransferProgress(msg.task as TransferTask);
  } else if (type === 'mcp:activity') {
    handleMcpActivity(msg);
  }
}

let aiPillTimer: number | null = null;
const aiTabTimerMap = new Map<string, number>();

function handleMcpActivity(msg: Record<string, unknown>): void {
  const tool = String(msg.tool || 'command');
  const host = String(msg.host || '');
  const sessionId = String(msg.sessionId || '');
  const pill = document.getElementById('ai-activity-pill');
  const textSpan = document.getElementById('ai-activity-text');
  if (pill && textSpan) {
    const hostHint = host ? ` · ${host}` : '';
    textSpan.textContent = `🤖 AI: [${tool}]${hostHint}`;
    pill.classList.remove('hidden');
    if (aiPillTimer !== null) {
      window.clearTimeout(aiPillTimer);
    }
    aiPillTimer = window.setTimeout(() => {
      pill.classList.add('hidden');
      aiPillTimer = null;
    }, 3500);
  }
  if (sessionId) {
    const tabElem = document.querySelector(`.tab[data-id="${sessionId}"]`);
    if (tabElem) {
      tabElem.classList.add('ai-active');
      const existingTimer = aiTabTimerMap.get(sessionId);
      if (existingTimer !== undefined) {
        window.clearTimeout(existingTimer);
      }
      const timer = window.setTimeout(() => {
        tabElem.classList.remove('ai-active');
        aiTabTimerMap.delete(sessionId);
      }, 3500);
      aiTabTimerMap.set(sessionId, timer);
    }
  }
}

interface QuickOpItem {
  icon: string;
  name: string;
  category: string;
  command: string;
  isBuiltin?: boolean;
  id?: string;
}

const BUILTIN_QUICK_OPS: QuickOpItem[] = [
  { icon: '📊', name: '系统负载与内存', category: '系统', command: 'uptime; free -h', isBuiltin: true },
  { icon: '⚡', name: 'CPU 占用 Top10', category: '排查', command: 'ps aux --sort=-%cpu | head -n 11', isBuiltin: true },
  { icon: '🧠', name: '内存占用 Top10', category: '排查', command: 'ps aux --sort=-%mem | head -n 11', isBuiltin: true },
  { icon: '🔌', name: '监听端口与进程', category: '网络', command: 'ss -tulpn 2>/dev/null || netstat -tulpn', isBuiltin: true },
  { icon: '🌐', name: '网卡与本机IP', category: '网络', command: 'ip -br a 2>/dev/null || ifconfig', isBuiltin: true },
  { icon: '🐳', name: 'Docker 容器状态', category: '容器', command: 'docker ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || podman ps', isBuiltin: true },
  { icon: '📜', name: '最新容器运行日志', category: '容器', command: 'docker ps -lq | xargs -r docker logs --tail 40', isBuiltin: true },
  { icon: '📦', name: 'Git 分支与状态', category: 'Git', command: 'git status -sb 2>/dev/null || echo "非 Git 仓库"', isBuiltin: true },
  { icon: '🌿', name: 'Git 最近提交树', category: 'Git', command: 'git log --oneline -n 10 --graph 2>/dev/null || echo "非 Git 仓库"', isBuiltin: true },
  { icon: '💾', name: '磁盘挂载与空间', category: '磁盘', command: 'df -hT', isBuiltin: true },
  { icon: '🔍', name: '当前目录大文件 Top10', category: '磁盘', command: 'du -ah -d 1 . 2>/dev/null | sort -rh | head -n 10', isBuiltin: true },
  { icon: '🚨', name: '系统核心错误日志', category: '系统', command: 'journalctl -p 3 -xb -n 30 --no-pager 2>/dev/null || dmesg | tail -n 30', isBuiltin: true },
];

async function renderQuickOpsBar(): Promise<void> {
  const container = document.getElementById('quick-ops-list');
  if (!container) return;
  container.textContent = '';

  let userSnippets: { id: string; name: string; category: string; command: string }[] = [];
  try {
    userSnippets = await webApi.listSnippets();
  } catch {
    // fallback
  }

  const items: QuickOpItem[] = [...BUILTIN_QUICK_OPS];
  for (const s of userSnippets) {
    items.push({
      icon: '⚡',
      name: s.name,
      category: s.category || '自定义',
      command: s.command,
      isBuiltin: false,
      id: s.id,
    });
  }

  for (const op of items) {
    const chip = document.createElement('div');
    chip.className = 'quick-chip';
    chip.title = `${op.name} [${op.category}]:\n${op.command}`;

    const iconSpan = document.createElement('span');
    iconSpan.className = 'chip-icon';
    iconSpan.textContent = op.icon;

    const nameSpan = document.createElement('span');
    nameSpan.className = 'chip-name';
    nameSpan.textContent = op.name;

    const catSpan = document.createElement('span');
    catSpan.className = 'chip-cat-badge';
    catSpan.textContent = op.category;

    chip.append(iconSpan, nameSpan, catSpan);
    if (op.id) {
      const del = document.createElement('button');
      del.type = 'button';
      del.className = 'chip-del';
      del.textContent = '×';
      del.title = `删除「${op.name}」`;
      del.setAttribute('aria-label', `删除快捷命令 ${op.name}`);
      del.addEventListener('click', (event) => {
        event.stopPropagation();
        void (async () => {
          const ok = await confirmAction(`删除快捷命令「${op.name}」？`, '这条命令会从本机的快捷命令里移除。');
          if (!ok || !op.id) return;
          try {
            await webApi.deleteSnippet(op.id);
            await renderQuickOpsBar();
          } catch (err) {
            showToast(err instanceof Error ? err.message : '删除快捷命令失败', 'error');
          }
        })();
      });
      chip.append(del);
    }
    chip.addEventListener('click', () => {
      if (!activeId) {
        showToast('请先打开或选中一个终端会话', 'error');
        return;
      }
      sendQuickCommand(op.command);
      document.getElementById('nav-ops-menu')?.classList.add('hidden');
      switchView('terminal');
    });
    container.appendChild(chip);
  }
}

async function refreshLive(kind: string): Promise<void> {
  if (kind === 'snippets' || kind === 'disk') {
    void renderQuickOpsBar();
  }
  if (kind === 'history' || kind === 'disk') await monCtrl.refreshHistory();
  if (kind === 'settings' || kind === 'disk') await applySettings();
  if (kind === 'tunnels' || kind === 'disk') monCtrl.refreshTunnelsIfOpen();
  if (kind === 'hosts' || kind === 'disk' || kind === '') await treeCtrl.refresh();
}

const ws = connectWs(handleWsEvent, () => { void refreshLive('disk'); });

function tabModels(): BroadcastTab[] {
  return Array.from(tabs.entries()).map(([id, tab]) => ({
    id,
    kind: tab.info.kind === 'local' ? 'local' : 'ssh',
    closed: tab.closed,
  }));
}

let lastLinkDownNotice = 0;

function dispatchInput(ids: string[], data: string): void {
  const delivery = inputDelivery(ids.length > 0, ws.readyState === WebSocket.OPEN);
  if (delivery === 'no-terminal') return;
  if (delivery === 'link-down') {
    const now = Date.now();
    if (data && shouldNotifyLinkDown(lastLinkDownNotice, now)) {
      lastLinkDownNotice = now;
      showToast('和本机工作台暂时断开，刚才的输入没有发出', 'error');
    }
    return;
  }
  if (ids.length === 1) {
    ws.send(JSON.stringify({ type: 'term:input', sessionId: ids[0], data }));
    return;
  }
  ws.send(JSON.stringify({ type: 'term:broadcast', sessionIds: ids, data }));
}

function inputRecipients(sourceId: string): string[] {
  return keystrokeRecipients(sourceId, tabModels(), Array.from(broadcastChecked), broadcastOn());
}

function sendQuickCommand(command: string): void {
  if (!activeId) return;
  const ids = inputRecipients(activeId);
  if (!ids.length) {
    showToast('当前终端已断开，命令没有发出', 'error');
    return;
  }
  if (!quickCommandInput(command)) {
    showToast('这条快捷命令是空的', 'error');
    return;
  }
  for (const id of ids) {
    const shell = tabs.get(id)?.info.shell === 'powershell' ? 'powershell' : 'posix';
    dispatchInput([id], quickCommandInput(command, shell));
  }
}

function sendInput(data: string): void {
  if (!activeId) return;
  const ids = inputRecipients(activeId);
  if (!ids.length) {
    showToast('当前终端已断开，命令没有发出', 'error');
    return;
  }
  dispatchInput(ids, data);
}

function sendToActive(data: string): void {
  dispatchInput(broadcastRecipients(activeId, tabModels(), [], false), data);
}

function fillCommand(cmd: string): void {
  const text = cmd.replace(/[\r\n]+$/, '');
  if (!text) return;
  switchView('terminal');
  document.getElementById('nav-ops-menu')?.classList.add('hidden');
  if (!activeId || tabs.get(activeId)?.closed) {
    showToast('先打开一个仍连接的终端，再把命令填进去', 'error');
    return;
  }
  sendToActive(text);
}

function showTerminalContextMenu(x: number, y: number, sessionId: string): void {
  const menu = document.getElementById('terminal-context-menu');
  const tab = tabs.get(sessionId);
  if (!menu || !tab) return;

  const copyItem = document.getElementById('ctx-copy');
  if (copyItem) {
    const hasSel = tab.term.hasSelection();
    copyItem.classList.toggle('disabled', !hasSel);
  }

  menu.classList.remove('hidden');
  const menuWidth = 200;
  const menuHeight = 220;
  const posX = x + menuWidth > window.innerWidth ? Math.max(10, window.innerWidth - menuWidth - 10) : x;
  const posY = y + menuHeight > window.innerHeight ? Math.max(10, window.innerHeight - menuHeight - 10) : y;
  menu.style.left = `${posX}px`;
  menu.style.top = `${posY}px`;
}

function confirmMultilinePaste(sessionId: string, text: string): void {
  const tab = tabs.get(sessionId);
  const dlg = document.getElementById('paste-confirm-dialog') as HTMLDialogElement | null;
  const form = document.getElementById('paste-confirm-form') as HTMLFormElement | null;
  if (!tab || !dlg || !form) return;
  const preview = document.getElementById('paste-confirm-preview') as HTMLElement | null;
  const hint = document.getElementById('paste-confirm-hint') as HTMLElement | null;
  const cancelBtn = document.getElementById('cancel-paste-btn') as HTMLButtonElement | null;
  const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (hint) hint.textContent = `检测到剪贴板包含 ${lines.length} 行文本/命令，直接粘贴将立即向终端提交执行：`;
  if (preview) preview.textContent = text;

  let settled = false;
  const finish = (choice: 'send' | 'dismiss') => {
    const action = settlePaste(settled, choice);
    if (action === 'ignore') return;
    settled = true;
    form.removeEventListener('submit', onConfirm);
    cancelBtn?.removeEventListener('click', onCancel);
    dlg.removeEventListener('cancel', onEscape);
    if (dlg.open) dlg.close();
    if (action === 'send') sendInput(text);
    tab.term.focus();
  };
  const onConfirm = (event: Event) => {
    event.preventDefault();
    finish('send');
  };
  const onCancel = () => finish('dismiss');
  const onEscape = (event: Event) => {
    event.preventDefault();
    finish('dismiss');
  };
  form.addEventListener('submit', onConfirm);
  cancelBtn?.addEventListener('click', onCancel);
  dlg.addEventListener('cancel', onEscape);
  dlg.showModal();
}

async function handleTerminalPaste(sessionId: string): Promise<void> {
  const tab = tabs.get(sessionId);
  if (!tab) return;
  let text = '';
  try {
    text = await navigator.clipboard.readText();
  } catch {
    showToast('无法读取系统剪贴板，请使用快捷键 Ctrl+V', 'error');
    return;
  }
  if (!text) return;
  if (pasteNeedsConfirm(text)) {
    confirmMultilinePaste(sessionId, text);
    return;
  }
  sendInput(text);
  tab.term.focus();
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

  // 划选即复制 (Copy-on-select)
  slot.addEventListener('mouseup', () => {
    const t = tabs.get(info.sessionId);
    if (t && t.term.hasSelection()) {
      const sel = t.term.getSelection();
      if (sel && sel.trim().length > 0) {
        void navigator.clipboard.writeText(sel).catch(() => {});
      }
    }
  });

  // 极客右键智能上下文菜单
  slot.addEventListener('paste', (event) => {
    const data = event.clipboardData;
    const text = data?.getData('text/plain') || data?.getData('text') || '';
    if (!pasteNeedsConfirm(text)) return;
    event.preventDefault();
    event.stopPropagation();
    confirmMultilinePaste(info.sessionId, text);
  }, true);
  slot.addEventListener('contextmenu', (e: MouseEvent) => {
    e.preventDefault();
    if (activeId !== info.sessionId) {
      selectTab(info.sessionId);
    }
    showTerminalContextMenu(e.clientX, e.clientY, info.sessionId);
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
    theme: termTheme(),
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(slot);
  const sourceId = info.sessionId;
  const dataSub = term.onData((data) => {
    dispatchInput(inputRecipients(sourceId), data);
  });
  tabs.set(info.sessionId, { info, term, fit, slot, metrics: null, closed: false, dataSub, lastResize: null });
  treeCtrl.render();
  selectTab(info.sessionId);
}

function isTerminalViewActive(): boolean {
  const viewTerm = document.getElementById('view-terminal');
  return Boolean(viewTerm && viewTerm.classList.contains('active'));
}

function fitTab(tab: TabState): void {
  if (!shouldMeasureTerminal(isTerminalViewActive(), tab.slot.classList.contains('hidden'))) return;

  const stage = document.getElementById('terminal-stage');
  if (!stage || stage.clientWidth < 100 || stage.clientHeight < 50) return;

  tab.fit.fit();
  const next = resizeIfChanged(tab.lastResize, tab.term.cols, tab.term.rows);
  if (!next || tab.closed || ws.readyState !== WebSocket.OPEN) return;
  tab.lastResize = next;
  ws.send(JSON.stringify({ type: 'term:resize', sessionId: tab.info.sessionId, cols: next.cols, rows: next.rows }));
}

function measureGrid(): { cols: number; rows: number } {
  const stage = document.getElementById('terminal-stage');
  if (!stage) return { cols: 100, rows: 28 };

  // 若已有活跃终端，直接复用其实际尺寸，避免向舞台注入临时 DOM 引起 Flex 布局抖动
  if (activeId) {
    const activeTab = tabs.get(activeId);
    if (activeTab && activeTab.term.cols >= 20 && activeTab.term.rows >= 5) {
      return { cols: activeTab.term.cols, rows: activeTab.term.rows };
    }
  }

  // 根据当前舞台可用几何像素估算标准字符网格（字符宽约 8.5px，行高约 17px）
  const stageWidth = stage.clientWidth;
  const stageHeight = stage.clientHeight;
  if (stageWidth > 100 && stageHeight > 50) {
    const estCols = Math.max(40, Math.min(300, Math.floor((stageWidth - 28) / 8.5)));
    const estRows = Math.max(10, Math.min(150, Math.floor((stageHeight - 24) / 17)));
    return { cols: estCols, rows: estRows };
  }

  return { cols: 100, rows: 28 };
}

function updateNavHostBadge(tab: TabState | undefined): void {
  const dot = document.getElementById('nav-host-dot');
  const aliasEl = document.getElementById('nav-host-alias');
  const ipEl = document.getElementById('nav-host-ip');
  if (!dot || !aliasEl || !ipEl) return;

  if (!tab) {
    dot.className = 'status-dot disconnected';
    aliasEl.textContent = '未连接主机';
    ipEl.textContent = '';
    return;
  }

  const who = tab.info.kind === 'local' ? '本机终端' : `${tab.info.username}@${tab.info.host}`;
  if (tab.closed) {
    dot.className = 'status-dot closed';
    aliasEl.textContent = tab.info.kind === 'local' ? '本机终端' : tab.info.alias;
    ipEl.textContent = `${who} 已断开`;
  } else {
    dot.className = 'status-dot connected';
    aliasEl.textContent = tab.info.kind === 'local' ? '本机终端' : tab.info.alias;
    ipEl.textContent = who;
  }
}

function updateSessionContext(): void {
  const el = document.getElementById('session-context');
  const tab = activeId ? tabs.get(activeId) : undefined;
  updateNavHostBadge(tab);
  if (!el) return;
  if (!tab) {
    el.textContent = '没有选中的会话';
    return;
  }
  const who = tab.info.kind === 'local' ? '本机终端' : `${tab.info.username}@${tab.info.host}`;
  let text = tab.closed ? `${who} · 已断开` : who;
  if (currentView === 'sftp' && tab.info.kind === 'ssh' && !tab.closed) {
    text += ` · ${sftpCtrl.currentPath()}`;
  }
  el.textContent = text;
}

function setViewTab(id: string, selected: boolean): void {
  const btn = document.getElementById(id);
  btn?.classList.toggle('active', selected);
  btn?.setAttribute('aria-selected', selected ? 'true' : 'false');
}

let isSftpSideSplit = false;

function toggleSftpSideSplit(force?: boolean): void {
  isSftpSideSplit = force !== undefined ? force : !isSftpSideSplit;
  if (isSftpSideSplit && currentView !== 'terminal') {
    switchView('terminal');
    return;
  }
  applySplitVisibility();
}

function applySplitVisibility(): void {
  const vis = resolveSplitVisibility({
    sftpSideSplit: isSftpSideSplit,
    currentView,
  });

  const viewStack = document.querySelector('.view-stack');
  const viewTerm = document.getElementById('view-terminal');
  const viewSftp = document.getElementById('view-sftp');
  const viewMon = document.getElementById('view-monitor');
  const actionText = document.getElementById('sftp-split-action-text');
  const splitIcon = document.getElementById('sftp-split-icon');
  const navSplitBtn = document.getElementById('nav-btn-split-toggle');

  if (actionText) actionText.textContent = vis.menuActionText;
  if (splitIcon) splitIcon.textContent = isSftpSideSplit ? '✕' : '◫';

  const isSplitActiveInView = vis.isSplitActive && currentView === 'terminal';
  viewStack?.classList.toggle('split-term-sftp', isSplitActiveInView);

  if (navSplitBtn) {
    navSplitBtn.classList.toggle('active', isSplitActiveInView);
    navSplitBtn.title = isSplitActiveInView
      ? '关闭并排分屏，还原单屏终端 (Ctrl+\\)'
      : '一键并排分屏：在右侧同屏展示文件管理 (Ctrl+\\)';
  }

  if (isSplitActiveInView) {
    viewTerm?.classList.add('active');
    viewSftp?.classList.add('active');
    viewMon?.classList.remove('active');
    setViewTab('nav-btn-terminal', true);
    setViewTab('nav-btn-sftp', true);
    setViewTab('nav-btn-monitor', false);
    sftpCtrl.syncSession();
  } else {
    viewTerm?.classList.toggle('active', vis.showTerminal);
    viewSftp?.classList.toggle('active', vis.showSftp);
    viewMon?.classList.toggle('active', vis.showMonitor);
    setViewTab('nav-btn-terminal', currentView === 'terminal');
    setViewTab('nav-btn-sftp', currentView === 'sftp');
    setViewTab('nav-btn-monitor', currentView === 'monitor');
  }

  requestAnimationFrame(() => {
    fitVisibleTabs();
    if (activeId) {
      const tab = tabs.get(activeId);
      if (tab && !tab.closed) {
        tab.term.focus();
        tab.term.refresh(0, tab.term.rows - 1);
      }
    }
  });
}

function switchView(view: ViewMode): void {
  currentView = view;
  document.querySelector('.app')?.setAttribute('data-view', view);
  applySplitVisibility();

  if (view === 'sftp' || (view === 'terminal' && isSftpSideSplit)) {
    sftpCtrl.syncSession();
  } else if (view === 'monitor') {
    const tab = activeId ? tabs.get(activeId) : undefined;
    showMetricsFor(tab);
  }
  updateSessionContext();
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
  broadcastChecked.delete(sessionId);
  tab.term.writeln('\r\n\x1b[33m[会话已断开]\x1b[0m');
  if (activeId === sessionId) {
    showMetricsFor(tab);
    sftpCtrl.syncSession();
    updateSessionContext();
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
  if (!isTerminalViewActive()) return;
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
        if (currentView === 'terminal') tab.term.focus();
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
    showIdleEmpty();
    monCtrl.renderMetrics(null, 'idle');
    renderTabs();
    sftpCtrl.syncSession();
    updateSessionContext();
    return;
  }

  const chosen = tabs.get(sessionId);
  if (chosen?.info.hostId) markSelectedHost(chosen.info.hostId);

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
        if (currentView === 'terminal') {
          tab.term.focus();
        }
        showMetricsFor(tab);
      }
    }
    renderTabs();
    sftpCtrl.syncSession();
  }
  updateSessionContext();
}

function broadcastOn(): boolean {
  return (document.getElementById('broadcast-toggle') as HTMLInputElement | null)?.checked ?? false;
}

function renderTabs(): void {
  const bar = document.getElementById('tabs') as HTMLElement;
  bar.textContent = '';
  for (const [id, tab] of tabs) {
    const isPrimary = id === activeId;
    const isSecondary = currentSplitMode !== 'single' && id === secondaryId;
    const chip = document.createElement('div');
    chip.className = `tab-chip${isPrimary ? ' active' : ''}${tab.closed ? ' closed' : ''}`;
    chip.setAttribute('role', 'tab');
    chip.setAttribute('aria-selected', isPrimary ? 'true' : 'false');
    if (broadcastOn() && !tab.closed) {
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.className = 'tab-bcast';
      const isCurrent = id === activeId;
      const local = tab.info.kind === 'local';
      box.title = isCurrent
        ? '当前终端总会收到'
        : local
          ? '勾选后，广播才会进入这个本机终端'
          : '勾选后，广播会进入这个会话';
      box.checked = isCurrent || broadcastChecked.has(id);
      box.disabled = isCurrent;
      box.addEventListener('click', (event) => event.stopPropagation());
      box.addEventListener('change', () => {
        if (box.checked) broadcastChecked.add(id);
        else broadcastChecked.delete(id);
      });
      chip.appendChild(box);
    }
    const dot = document.createElement('span');
    dot.className = 'tab-dot';
    const label = document.createElement('span');
    label.textContent = tab.info.kind === 'local' ? '本机' : (sessionTabLabels().get(id) ?? tab.info.alias);
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

    if (tab.closed && tab.info.kind === 'ssh' && tab.info.hostId) {
      const reconnect = document.createElement('button');
      reconnect.type = 'button';
      reconnect.className = 'tab-reconnect';
      reconnect.textContent = '重连';
      const closedId = id;
      reconnect.addEventListener('click', (event) => {
        event.stopPropagation();
        void reconnectClosedTab(closedId);
      });
      chip.appendChild(reconnect);
    }

    const closeBtn = document.createElement('button');
    closeBtn.type = 'button';
    closeBtn.textContent = '×';
    closeBtn.setAttribute('aria-label', '关闭会话');
    closeBtn.style.padding = '0 4px';
    closeBtn.addEventListener('click', (e) => { e.stopPropagation(); void closeTab(id); });
    chip.appendChild(closeBtn);
    chip.addEventListener('click', () => selectTab(id));
    bar.appendChild(chip);
  }
  for (const [hostId, pending] of pendingConnections) {
    const chip = document.createElement('div');
    chip.className = 'tab-chip connecting active';
    const dot = document.createElement('span');
    dot.className = 'tab-dot spin';
    const label = document.createElement('span');
    label.textContent = `${pending.alias}（${pending.detail}）`;
    chip.append(dot, label);
    if (!pending.cancelled) {
      const cancel = document.createElement('button');
      cancel.type = 'button';
      cancel.className = 'tab-reconnect';
      cancel.textContent = '取消';
      cancel.addEventListener('click', (event) => {
        event.stopPropagation();
        cancelConnect(hostId);
      });
      chip.appendChild(cancel);
    }
    bar.appendChild(chip);
  }
}

async function disposeTab(sessionId: string, selectFallback: boolean): Promise<void> {
  const tab = tabs.get(sessionId);
  if (!tab) return;
  try {
    await webApi.closeSession(sessionId);
  } catch {
    // 会话可能已经在服务端断开。
  }
  tab.dataSub.dispose();
  tab.term.dispose();
  tab.slot.remove();
  tabs.delete(sessionId);
  broadcastChecked.delete(sessionId);
  if (secondaryId === sessionId) secondaryId = null;
  if (tabs.size < 2 && currentSplitMode !== 'single') {
    currentSplitMode = 'single';
    updateSplitControls();
    document.getElementById('terminal-stage')?.classList.remove('split-h', 'split-v');
  }
  treeCtrl.render();
  if (activeId === sessionId) {
    if (selectFallback) selectTab(Array.from(tabs.keys()).pop() ?? null);
    else {
      activeId = null;
      renderTabs();
      updateSessionContext();
      if (tabs.size === 0) showIdleEmpty();
    }
  } else if (selectFallback) {
    updateSplitLayout();
  } else {
    renderTabs();
  }
}

async function closeTab(sessionId: string): Promise<void> {
  await disposeTab(sessionId, true);
}

function jumpPhrase(hostId: string): string {
  const host = treeCtrl.hosts.find((item) => item.id === hostId);
  if (!host?.jumpHostId) return '';
  const jump = treeCtrl.hosts.find((item) => item.id === host.jumpHostId);
  return jump ? `经跳板机 ${jump.alias}` : '经跳板机';
}

function showIdleEmpty(): void {
  const el = document.getElementById('term-empty');
  if (!el) return;
  el.classList.remove('hidden');
  el.replaceChildren();
  const title = document.createElement('p');
  title.className = 'empty-kicker';
  title.textContent = '没有打开的终端';
  const sub = document.createElement('p');
  sub.textContent = '双击左侧主机，或使用上方「本机终端」。';
  el.append(title, sub);
}

function showConnectEmpty(title: string, sub: string, isError = false): void {
  if (tabs.size > 0) return;
  const el = document.getElementById('term-empty');
  if (!el) return;
  el.classList.remove('hidden');
  el.replaceChildren();
  const heading = document.createElement('p');
  heading.className = 'empty-kicker';
  heading.textContent = title;
  const detail = document.createElement('p');
  if (isError) detail.className = 'empty-error';
  detail.textContent = sub;
  el.append(heading, detail);
}

function cancelConnect(hostId: string): void {
  const pending = pendingConnections.get(hostId);
  if (!pending || pending.cancelled) return;
  pending.cancelled = true;
  pending.detail = '正在取消';
  cancelledHostIds.add(hostId);
  pending.abort.abort();
  renderTabs();
  treeCtrl.render();
}

function sessionTabLabels(): Map<string, string> {
  return tabLabels(Array.from(tabs.values()).map((tab) => ({
    id: tab.info.sessionId,
    alias: tab.info.alias,
    hostId: tab.info.hostId ?? null,
  })));
}

async function reconnectClosedTab(sessionId: string): Promise<void> {
  const tab = tabs.get(sessionId);
  const hostId = tab?.info.hostId;
  if (!tab?.closed || !hostId) return;
  if (pendingConnections.has(hostId)) return;
  await disposeTab(sessionId, true);
  await openSshTab(hostId, currentView === 'terminal', 'another');
}

async function openSshTab(hostId: string, showTerminal: boolean, intent: SessionOpenIntent = 'reuse'): Promise<void> {
  const plan = planSessionOpen(
    hostId,
    Array.from(tabs.values()).map((tab) => ({
      id: tab.info.sessionId,
      hostId: tab.info.hostId ?? null,
      closed: tab.closed,
    })),
    pendingConnections.has(hostId),
    intent,
  );
  const steps = sessionOpenSteps(plan);
  if (steps[0] === 'focus' && plan.focusId) {
    selectTab(plan.focusId);
    return;
  }
  if (!steps.includes('connect')) return;

  cancelledHostIds.delete(hostId);
  const host = treeCtrl.hosts.find((item) => item.id === hostId);
  const alias = host?.alias ?? '远程主机';
  const via = jumpPhrase(hostId);
  const detail = via ? `正在连接，${via}` : '正在连接';
  const abort = new AbortController();
  pendingConnections.set(hostId, { alias, detail, abort, cancelled: false });
  hostErrors.delete(hostId);
  selectedHostId = hostId;
  if (showTerminal) switchView('terminal');
  const target = host ? `${host.username}@${host.host}:${host.port}` : alias;
  showConnectEmpty(`正在连接 ${alias}`, via ? `${target} · ${via}` : target);
  renderTabs();
  treeCtrl.render();

  try {
    if (steps.includes('dispose') && plan.disposeId) {
      await disposeTab(plan.disposeId, true);
      if (abort.signal.aborted || pendingConnections.get(hostId)?.cancelled) {
        pendingConnections.delete(hostId);
        cancelledHostIds.delete(hostId);
        renderTabs();
        treeCtrl.render();
        if (tabs.size === 0) showIdleEmpty();
        return;
      }
    }
    const grid = measureGrid();
    const info = (await webApi.openSession({ kind: 'ssh', hostId, cols: grid.cols, rows: grid.rows }, abort.signal)) as ConnectInfo & { hostId?: string };
    if (!info.hostId) info.hostId = hostId;
    const pending = pendingConnections.get(hostId);
    if (pending?.cancelled || cancelledHostIds.has(hostId) || abort.signal.aborted) {
      pendingConnections.delete(hostId);
      cancelledHostIds.delete(hostId);
      await webApi.closeSession(info.sessionId).catch(() => undefined);
      renderTabs();
      treeCtrl.render();
      if (tabs.size === 0) showIdleEmpty();
      return;
    }
    pendingConnections.delete(hostId);
    hostErrors.delete(hostId);
    createTermInstance(info);
    const tab = tabs.get(info.sessionId);
    if (tab) requestAnimationFrame(() => fitTab(tab));
  } catch (err) {
    const pending = pendingConnections.get(hostId);
    pendingConnections.delete(hostId);
    if (abort.signal.aborted || pending?.cancelled) {
      renderTabs();
      treeCtrl.render();
      if (tabs.size === 0) showIdleEmpty();
      return;
    }
    const raw = err instanceof Error ? err.message : '连接失败';
    const message = via && !raw.includes('跳板') ? `${via} 连接失败：${raw}` : raw;
    hostErrors.set(hostId, message);
    showConnectEmpty(`连接 ${alias} 失败`, message, true);
    showToast(message, 'error');
    renderTabs();
    treeCtrl.render();
  }
}

let findNeedle = '';
let findSessionId: string | null = null;
let findCursor = -1;
let findHits: Array<{ row: number; col: number }> = [];

function setFindCount(text: string, title = ''): void {
  const el = document.getElementById('search-count');
  if (!el) return;
  el.textContent = text;
  el.title = title;
}

function collectHits(tab: TabState, needle: string): Array<{ row: number; col: number }> {
  const hits: Array<{ row: number; col: number }> = [];
  const buf = tab.term.buffer.active;
  const lowered = needle.toLowerCase();
  for (let row = 0; row < buf.length; row += 1) {
    const line = (buf.getLine(row)?.translateToString() ?? '').toLowerCase();
    let from = 0;
    while (from < line.length) {
      const col = line.indexOf(lowered, from);
      if (col === -1) break;
      hits.push({ row, col });
      from = col + Math.max(1, lowered.length);
    }
  }
  return hits;
}

function findInActiveTerm(needle: string, forward = true): void {
  const trimmed = needle.trim();
  if (!activeId || !trimmed) {
    setFindCount('');
    return;
  }
  const tab = tabs.get(activeId);
  if (!tab) return;
  if (trimmed !== findNeedle || activeId !== findSessionId) {
    findNeedle = trimmed;
    findSessionId = activeId;
    findHits = collectHits(tab, trimmed);
    findCursor = -1;
  }
  if (!findHits.length) {
    setFindCount('0/0', '没有匹配');
    return;
  }
  const previous = findCursor;
  findCursor = nextFindIndex(findCursor, findHits.length, forward);
  const hit = findHits[findCursor];
  if (!hit) return;
  tab.term.scrollToLine(Math.max(0, hit.row - 3));
  tab.term.select(hit.col, hit.row, trimmed.length);
  const wrapped = previous >= 0 && ((forward && findCursor < previous) || (!forward && findCursor > previous));
  setFindCount(`${findCursor + 1}/${findHits.length}`, wrapped ? '已从另一头继续' : '');
}

function closeFind(): void {
  document.getElementById('search-bar')?.classList.add('hidden');
  if (currentView === 'terminal' && activeId) tabs.get(activeId)?.term.focus();
}

function openFind(): void {
  switchView('terminal');
  const bar = document.getElementById('search-bar');
  bar?.classList.remove('hidden');
  const input = document.getElementById('search-input') as HTMLInputElement | null;
  input?.focus();
  input?.select();
}

function bindGlobalUi(): void {
  // 核心三合一全局视图切换
  document.getElementById('nav-btn-terminal')?.addEventListener('click', () => switchView('terminal'));
  document.getElementById('nav-btn-sftp')?.addEventListener('click', () => switchView('sftp'));
  document.getElementById('nav-btn-monitor')?.addEventListener('click', () => switchView('monitor'));

  // 文件 Tab 右键分屏联动与快捷菜单
  const navBtnSftp = document.getElementById('nav-btn-sftp');
  const navSftpMenu = document.getElementById('nav-sftp-menu');
  const sftpSplitToggleItem = document.getElementById('sftp-split-toggle-item');
  const sftpSplitClose = document.getElementById('sftp-split-close');

  navBtnSftp?.addEventListener('contextmenu', (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const actionText = document.getElementById('sftp-split-action-text');
    const splitIcon = document.getElementById('sftp-split-icon');
    if (actionText) actionText.textContent = isSftpSideSplit ? '关闭右侧文件分屏' : '在右侧分屏打开文件';
    if (splitIcon) splitIcon.textContent = isSftpSideSplit ? '✕' : '◫';
    navSftpMenu?.classList.toggle('hidden');
  });

  sftpSplitToggleItem?.addEventListener('click', () => {
    navSftpMenu?.classList.add('hidden');
    toggleSftpSideSplit();
  });

  sftpSplitClose?.addEventListener('click', () => {
    toggleSftpSideSplit(false);
  });

  window.addEventListener('click', (e: MouseEvent) => {
    if (!navSftpMenu?.contains(e.target as Node) && e.target !== navBtnSftp) {
      navSftpMenu?.classList.add('hidden');
    }
  });

  const navBtnSplitToggle = document.getElementById('nav-btn-split-toggle');
  navBtnSplitToggle?.addEventListener('click', () => {
    toggleSftpSideSplit();
  });

  window.addEventListener('keydown', (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      navSftpMenu?.classList.add('hidden');
    } else if ((e.ctrlKey || e.metaKey) && e.key === '\\') {
      e.preventDefault();
      toggleSftpSideSplit();
    }
  });

  // 顶部导航：悬浮运维动作下拉及 Tab 切换
  const navOpsToggle = document.getElementById('nav-ops-toggle');
  const navOpsMenu = document.getElementById('nav-ops-menu');
  const tabSnippets = document.getElementById('ops-tab-snippets');
  const tabHistory = document.getElementById('ops-tab-history');
  const viewSnippets = document.getElementById('ops-view-snippets');
  const viewHistory = document.getElementById('ops-view-history');

  function switchOpsTab(tab: 'snippets' | 'history') {
    tabSnippets?.classList.toggle('active', tab === 'snippets');
    tabSnippets?.setAttribute('aria-selected', tab === 'snippets' ? 'true' : 'false');
    tabHistory?.classList.toggle('active', tab === 'history');
    tabHistory?.setAttribute('aria-selected', tab === 'history' ? 'true' : 'false');
    viewSnippets?.classList.toggle('active', tab === 'snippets');
    viewHistory?.classList.toggle('active', tab === 'history');
    if (tab === 'history') {
      void monCtrl.refreshHistory();
    }
  }

  tabSnippets?.addEventListener('click', (e) => {
    e.stopPropagation();
    switchOpsTab('snippets');
  });
  tabHistory?.addEventListener('click', (e) => {
    e.stopPropagation();
    switchOpsTab('history');
  });

  navOpsToggle?.addEventListener('click', (e) => {
    e.stopPropagation();
    const open = navOpsMenu?.classList.toggle('hidden') === false;
    navOpsToggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (open && tabHistory?.classList.contains('active')) {
      void monCtrl.refreshHistory();
    }
  });
  const moreBtn = document.getElementById('nav-more');
  const moreWrap = moreBtn?.parentElement;
  moreBtn?.addEventListener('click', (event) => {
    event.stopPropagation();
    const open = moreWrap?.classList.toggle('open') ?? false;
    moreBtn.setAttribute('aria-expanded', open ? 'true' : 'false');
  });
  window.addEventListener('click', (e) => {
    if (navOpsMenu && !navOpsMenu.classList.contains('hidden') && !navOpsMenu.contains(e.target as Node) && e.target !== navOpsToggle) {
      navOpsMenu.classList.add('hidden');
      navOpsToggle?.setAttribute('aria-expanded', 'false');
    }
    if (moreWrap?.classList.contains('open') && !moreWrap.contains(e.target as Node)) {
      moreWrap.classList.remove('open');
      moreBtn?.setAttribute('aria-expanded', 'false');
    }
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && navOpsMenu && !navOpsMenu.classList.contains('hidden')) {
      navOpsMenu.classList.add('hidden');
    }
  });

  // 沉浸式窗口三钮与顶栏拖拽 (Wry IPC 桥接)
  document.getElementById('win-min')?.addEventListener('click', () => {
    (window as any).ipc?.postMessage('min');
  });
  document.getElementById('win-max')?.addEventListener('click', () => {
    (window as any).ipc?.postMessage('max');
  });
  document.getElementById('win-close')?.addEventListener('click', () => {
    (window as any).ipc?.postMessage('close');
  });

  const globalNav = document.querySelector('.global-nav');
  globalNav?.addEventListener('mousedown', (e: Event) => {
    const mouseEvent = e as MouseEvent;
    if (mouseEvent.button !== 0) return;
    const target = mouseEvent.target as HTMLElement | null;
    if (!target) return;
    if (target.closest('button, input, select, textarea, .segmented-control, .ops-menu, .nav-host-badge, .toggle-chip, .split-controls')) {
      return;
    }
    (window as any).ipc?.postMessage('drag');
  });

  globalNav?.addEventListener('dblclick', (e: Event) => {
    const target = (e as MouseEvent).target as HTMLElement | null;
    if (!target || target.closest('button, input, select, textarea, .segmented-control, .ops-menu, .nav-host-badge, .toggle-chip, .split-controls')) {
      return;
    }
    (window as any).ipc?.postMessage('max');
  });

  // 左侧栏收起 / 展开
  const sidebar = document.getElementById('sidebar');
  const sideToggleBtn = document.getElementById('sidebar-toggle-btn');
  sideToggleBtn?.addEventListener('click', () => {
    if (!sidebar) return;
    const isCollapsed = sidebar.classList.toggle('collapsed');
    sideToggleBtn.textContent = isCollapsed ? '▶' : '◀';
    sideToggleBtn.title = isCollapsed ? '展开左侧栏' : '收起左侧栏';
    sideToggleBtn.setAttribute('aria-label', sideToggleBtn.title);
    requestAnimationFrame(() => fitVisibleTabs());
  });

  // 顶部活动主机指示徽标点击联动切换至当前活跃终端
  document.getElementById('nav-host-badge')?.addEventListener('click', () => {
    if (activeId) switchView('terminal');
  });

  // 打开新终端（顶部栏与侧栏）
  let localOpenInFlight = false;
  const handleOpenLocal = async () => {
    if (!claimSingleFlight(localOpenInFlight)) return;
    localOpenInFlight = true;
    try {
      const grid = measureGrid();
      createTermInstance(await webApi.openSession({ kind: 'local', cols: grid.cols, rows: grid.rows }));
    } catch (err) {
      showToast(err instanceof Error ? err.message : '本机终端没有打开', 'error');
    } finally {
      localOpenInFlight = false;
    }
  };
  document.getElementById('new-tab-local')?.addEventListener('click', () => void handleOpenLocal());

  document.getElementById('split-single')?.addEventListener('click', () => setSplitMode('single'));
  document.getElementById('split-horizontal')?.addEventListener('click', () => setSplitMode('horizontal'));
  document.getElementById('split-vertical')?.addEventListener('click', () => setSplitMode('vertical'));
  document.getElementById('broadcast-toggle')?.addEventListener('change', () => {
    broadcastChecked.clear();
    if (broadcastOn()) {
      for (const id of seedBroadcastChecks(tabModels())) broadcastChecked.add(id);
    }
    renderTabs();
  });
  document.getElementById('open-tunnels')?.addEventListener('click', () => void monCtrl.openTunnelModal());
  document.getElementById('theme-toggle')?.addEventListener('click', async () => {
    const next = document.documentElement.getAttribute('data-theme') === 'light' ? 'dark' : 'light';
    document.documentElement.setAttribute('data-theme', next);
    syncThemeToggle();
    applyTermTheme();
    await webApi.updateSettings({ theme: next });
    moreWrap?.classList.remove('open');
  });
  document.getElementById('search-toggle')?.addEventListener('click', () => openFind());
  document.getElementById('search-close')?.addEventListener('click', () => closeFind());
  document.getElementById('search-prev')?.addEventListener('click', () => findInActiveTerm((document.getElementById('search-input') as HTMLInputElement).value, false));
  document.getElementById('search-next')?.addEventListener('click', () => findInActiveTerm((document.getElementById('search-input') as HTMLInputElement).value, true));
  document.getElementById('search-input')?.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    findInActiveTerm((event.target as HTMLInputElement).value, !event.shiftKey);
  });

  const setDlg = document.getElementById('settings-dialog') as HTMLDialogElement;
  document.getElementById('open-settings')?.addEventListener('click', () => {
    setDlg.showModal();
    moreWrap?.classList.remove('open');
  });
  document.getElementById('cancel-settings')?.addEventListener('click', () => setDlg.close());
  document.getElementById('settings-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    try {
      const s = await webApi.updateSettings({
        theme: (document.getElementById('set-theme') as HTMLSelectElement).value as 'dark' | 'light',
        fontSize: Number((document.getElementById('set-font') as HTMLInputElement).value),
        monitorIntervalMs: Number((document.getElementById('set-interval') as HTMLInputElement).value),
        showHiddenFiles: (document.getElementById('set-hidden') as HTMLInputElement).checked,
      });
      fontSize = s.fontSize;
      document.documentElement.setAttribute('data-theme', s.theme);
      syncThemeToggle();
      applyTermTheme();
      sftpCtrl.setShowHidden(s.showHiddenFiles);
      for (const tab of tabs.values()) tab.term.options.fontSize = fontSize;
      fitVisibleTabs();
      setDlg.close();
    } catch (err) {
      showToast(err instanceof Error ? err.message : '设置没有保存', 'error');
    }
  });

  // Terminal Custom Context Menu Actions
  document.getElementById('ctx-copy')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    if (!activeId) return;
    const tab = tabs.get(activeId);
    if (tab && tab.term.hasSelection()) {
      const sel = tab.term.getSelection();
      void copyText(sel, `已复制 ${sel.length} 字符`);
    }
  });

  document.getElementById('ctx-paste')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    if (activeId) void handleTerminalPaste(activeId);
  });

  document.getElementById('ctx-copy-all')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    if (!activeId) return;
    const tab = tabs.get(activeId);
    if (!tab) return;
    const buf = tab.term.buffer.active;
    let text = '';
    for (let i = 0; i < buf.length; i++) {
      const line = buf.getLine(i);
      if (line) text += line.translateToString(true) + '\n';
    }
    void copyText(text.trimEnd(), '已复制当前终端全部内容');
  });

  document.getElementById('ctx-clear')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    sendInput(clearScreenInput());
  });

  document.getElementById('ctx-search')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    openFind();
  });

  document.getElementById('ctx-select-all')?.addEventListener('click', () => {
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    if (!activeId) return;
    tabs.get(activeId)?.term.selectAll();
  });

  window.addEventListener('click', (e) => {
    const menu = document.getElementById('terminal-context-menu');
    if (menu && !menu.classList.contains('hidden') && !menu.contains(e.target as Node)) {
      menu.classList.add('hidden');
    }
  });
  window.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    const el = document.activeElement as HTMLElement | null;
    if (el?.closest('dialog')) return;
    document.getElementById('terminal-context-menu')?.classList.add('hidden');
    if (el?.closest('#search-bar')) {
      e.preventDefault();
      closeFind();
    }
  });
}

window.addEventListener('keydown', (e) => {
  if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'f') return;
  const el = document.activeElement as HTMLElement | null;
  if (!el || el.closest('dialog')) return;
  if (!el.closest('#search-bar, #terminal-stage, .xterm')) return;
  e.preventDefault();
  openFind();
});

window.addEventListener('resize', () => {
  if (isTerminalViewActive()) fitVisibleTabs();
});
const stage = document.getElementById('terminal-stage');
if (stage && 'ResizeObserver' in window) {
  const observer = new ResizeObserver(() => {
    if (isTerminalViewActive()) fitVisibleTabs();
  });
  observer.observe(stage);
}
void document.fonts?.ready.then(() => {
  if (isTerminalViewActive()) fitVisibleTabs();
});

async function applySettings(): Promise<void> {
  const s = await webApi.getSettings();
  fontSize = s.fontSize;
  document.documentElement.setAttribute('data-theme', s.theme);
  syncThemeToggle();
  applyTermTheme();
  sftpCtrl.setShowHidden(s.showHiddenFiles);
  const theme = document.getElementById('set-theme') as HTMLSelectElement | null;
  const font = document.getElementById('set-font') as HTMLInputElement | null;
  const interval = document.getElementById('set-interval') as HTMLInputElement | null;
  const hidden = document.getElementById('set-hidden') as HTMLInputElement | null;
  if (theme) theme.value = s.theme;
  if (font) font.value = String(s.fontSize);
  if (interval) interval.value = String(s.monitorIntervalMs);
  if (hidden) hidden.checked = s.showHiddenFiles;
  for (const tab of tabs.values()) tab.term.options.fontSize = fontSize;
  for (const tab of tabs.values()) tab.lastResize = null;
  fitVisibleTabs();
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
    showToast('复制失败，请在对话框里手动选择', 'error');
  }
}

async function writeCursorMcp(): Promise<void> {
  try {
    const ok = await confirmAction(
      '一键同步 AI 客户端 MCP 配置？',
      '将自动检测并同步更新用户目录下的 TianCode (~/.tiancode/mcp.json) 与 Cursor (~/.cursor/mcp.json) 配置。其他 MCP 服务保持原样，写入后仅当前用户可读。',
      '立即同步',
    );
    if (!ok) return;
    const result = await webApi.writeCursorMcp();
    const synced = (result as { synced?: Array<{ client: string; action: string }> }).synced;
    if (synced && synced.length > 0) {
      const summary = synced.map((s) => `${s.client}: ${s.action === 'unchanged' ? '最新' : '已同步'}`).join('，');
      showToast(`已完成 AI 客户端同步 (${summary})`);
      return;
    }
    const verb = result.action === 'created' ? '已创建' : result.action === 'updated' ? '已更新' : '已是最新';
    showToast(`${verb} ${result.path}`);
  } catch (err) {
    showToast(err instanceof Error ? err.message : '写入 MCP 配置失败', 'error');
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
  switchView('terminal');
  syncThemeToggle();
  updateSessionContext();
  document.getElementById('open-mcp')?.addEventListener('click', () => void showMcpDialog().catch((err: unknown) => {
    showToast(err instanceof Error ? err.message : '读不到 MCP 地址');
  }));
  document.getElementById('close-mcp')?.addEventListener('click', () => {
    (document.getElementById('mcp-dialog') as HTMLDialogElement).close();
  });
  document.getElementById('mcp-token-toggle')?.addEventListener('click', () => {
    const input = document.getElementById('mcp-token') as HTMLInputElement;
    const hidden = input.type === 'password';
    input.type = hidden ? 'text' : 'password';
    const btn = document.getElementById('mcp-token-toggle');
    if (btn) btn.textContent = hidden ? '隐藏' : '显示';
  });
  document.getElementById('mcp-token-copy')?.addEventListener('click', () => {
    void copyText((document.getElementById('mcp-token') as HTMLInputElement).value, '已复制令牌');
  });
  document.getElementById('copy-cursor')?.addEventListener('click', () => {
    void copyText((document.getElementById('mcp-cursor') as HTMLTextAreaElement).value, '已复制 Cursor 配置');
  });
  document.getElementById('write-cursor-mcp')?.addEventListener('click', () => {
    void writeCursorMcp();
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
  void renderQuickOpsBar();
}

void boot();
