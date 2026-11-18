import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import type { TianshellApi } from '../main/preload.js';
import type { AuthType, HostDraft, PublicHost } from '../shared/types.js';

declare global {
  interface Window {
    tianshell: TianshellApi;
  }
}

const api = window.tianshell;
const term = new Terminal({
  fontFamily: '"Cascadia Code", "Sarasa Mono SC", "JetBrains Mono", Consolas, monospace',
  fontSize: 14,
  cursorBlink: true,
  scrollback: 5000,
  theme: {
    background: '#101418',
    foreground: '#e7e1d5',
    cursor: '#e3a008',
    cursorAccent: '#101418',
    selectionBackground: '#3d4a38',
    black: '#1c2229',
    red: '#e15a4a',
    green: '#7dba6a',
    yellow: '#e3a008',
    blue: '#79b8ff',
    magenta: '#d18adf',
    cyan: '#3dbea5',
    white: '#e7e1d5',
    brightBlack: '#5c6770',
    brightWhite: '#fff8ea',
  },
});
const fit = new FitAddon();
term.loadAddon(fit);

const terminalEl = mustGet<HTMLDivElement>('terminal');
term.open(terminalEl);
term.writeln('TianShell');
term.writeln('选一台主机再连接。登录信息只留在这台电脑上。');

let hosts: PublicHost[] = [];
let selectedId: string | null = null;
let sessionId: string | null = null;
let phase: 'idle' | 'connecting' | 'connected' | 'error' = 'idle';
let editing: PublicHost | null = null;

const hostList = mustGet<HTMLUListElement>('host-list');
const filterInput = mustGet<HTMLInputElement>('filter');
const connectBtn = mustGet<HTMLButtonElement>('connect');
const disconnectBtn = mustGet<HTMLButtonElement>('disconnect');
const dialog = mustGet<HTMLDialogElement>('host-dialog');
const confirmDialog = mustGet<HTMLDialogElement>('confirm-dialog');
const form = mustGet<HTMLFormElement>('host-form');

api.onData((id, data) => {
  if (id === sessionId) term.write(asBytes(data));
});
api.onClosed((id) => {
  if (id !== sessionId) return;
  sessionId = null;
  setPhase('idle', '连接已断开', '可以重新连接，或换一台主机');
  term.writeln('\r\n\x1b[33m连接已断开\x1b[0m');
});
api.onAddHost(() => openDialog(null));

mustGet<HTMLButtonElement>('add-host').addEventListener('click', () => openDialog(null));
connectBtn.addEventListener('click', () => {
  if (selectedId) void connect(selectedId);
});
disconnectBtn.addEventListener('click', () => void disconnect());
filterInput.addEventListener('input', renderHosts);
mustGet<HTMLButtonElement>('cancel-host').addEventListener('click', () => dialog.close());
form.addEventListener('submit', (event) => {
  event.preventDefault();
  void saveForm();
});
mustGet<HTMLButtonElement>('pick-key').addEventListener('click', () => void pickKey());

for (const input of Array.from(form.querySelectorAll<HTMLInputElement>('input[name="auth"]'))) {
  input.addEventListener('change', syncAuthFields);
}

document.querySelectorAll<HTMLButtonElement>('.tab').forEach((tab) => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach((item) => item.classList.remove('active'));
    tab.classList.add('active');
    const name = tab.dataset.tab;
    mustGet('panel-term').classList.toggle('hidden', name !== 'term');
    mustGet('panel-sftp').classList.toggle('hidden', name !== 'sftp');
    if (name === 'term') requestAnimationFrame(() => fit.fit());
  });
});

const observer = new ResizeObserver(() => {
  fit.fit();
  if (sessionId) api.resize(sessionId, term.cols, term.rows);
});
observer.observe(terminalEl);

term.onData((data) => {
  if (sessionId) api.write(sessionId, data);
});
term.onResize(({ cols, rows }) => {
  if (sessionId) api.resize(sessionId, cols, rows);
});

void boot();

async function boot(): Promise<void> {
  const dir = await api.dataDir();
  const dirEl = mustGet<HTMLElement>('data-dir');
  if (dir.ok) {
    dirEl.textContent = dir.data;
    dirEl.title = `主机数据在 ${dir.data}`;
  }
  await refresh();
}

async function refresh(): Promise<void> {
  const result = await api.listHosts();
  if (!result.ok) {
    toast(result.error);
    return;
  }
  hosts = result.data;
  if (selectedId && !hosts.some((host) => host.id === selectedId)) selectedId = hosts[0]?.id ?? null;
  if (!selectedId && hosts[0]) selectedId = hosts[0].id;
  renderHosts();
  paintStatus();
}

function renderHosts(): void {
  const query = filterInput.value.trim().toLowerCase();
  const visible = hosts.filter((host) => {
    const haystack = `${host.alias} ${host.username}@${host.host}:${host.port}`.toLowerCase();
    return haystack.includes(query);
  });
  hostList.replaceChildren();
  if (visible.length === 0) {
    const empty = document.createElement('li');
    empty.className = 'empty';
    empty.textContent = hosts.length === 0 ? '还没有主机。加一台你常连的服务器就行。' : '没有符合筛选的主机。';
    hostList.append(empty);
    return;
  }
  for (const host of visible) {
    const item = document.createElement('li');
    item.className = host.id === selectedId ? 'host selected' : 'host';
    const select = document.createElement('button');
    select.type = 'button';
    select.className = 'host-select';
    const alias = document.createElement('span');
    alias.className = 'host-alias';
    alias.textContent = host.alias;
    const meta = document.createElement('span');
    meta.className = 'host-meta';
    meta.textContent = `${host.username}@${host.host}:${host.port}`;
    select.append(alias, meta);
    select.addEventListener('click', () => {
      selectedId = host.id;
      renderHosts();
      paintStatus();
    });
    select.addEventListener('dblclick', () => void connect(host.id));

    const actions = document.createElement('div');
    actions.className = 'host-actions';
    actions.append(
      smallButton('连接', () => void connect(host.id)),
      smallButton('编辑', () => openDialog(host)),
      smallButton('删除', () => void removeHost(host)),
    );
    item.append(select, actions);
    hostList.append(item);
  }
}

async function connect(hostId: string): Promise<void> {
  selectedId = hostId;
  renderHosts();
  if (phase === 'connecting') return;
  if (sessionId) await disconnect();
  const host = hosts.find((item) => item.id === hostId);
  setPhase('connecting', '正在连接', host ? host.alias : '');
  term.reset();
  term.writeln(`\x1b[33m正在连接 ${host?.alias ?? ''}…\x1b[0m`);
  fit.fit();
  const result = await api.connect(hostId, term.cols, term.rows);
  if (!result.ok) {
    setPhase('error', '连接失败', result.error);
    term.writeln(`\r\n\x1b[31m${result.error}\x1b[0m`);
    toast(result.error);
    return;
  }
  sessionId = result.data.sessionId;
  setPhase('connected', result.data.alias, `${result.data.username}@${result.data.host}:${result.data.port}`);
  api.resize(sessionId, term.cols, term.rows);
  term.focus();
}

async function disconnect(): Promise<void> {
  const current = sessionId;
  if (!current) return;
  sessionId = null;
  await api.disconnect(current);
  setPhase('idle', '未连接', '从左边选一台主机');
}

function openDialog(host: PublicHost | null): void {
  editing = host;
  mustGet<HTMLElement>('dialog-title').textContent = host ? '编辑主机' : '添加主机';
  mustGet<HTMLInputElement>('field-alias').value = host?.alias ?? '';
  mustGet<HTMLInputElement>('field-host').value = host?.host ?? '';
  mustGet<HTMLInputElement>('field-port').value = String(host?.port ?? 22);
  mustGet<HTMLInputElement>('field-user').value = host?.username ?? '';
  mustGet<HTMLInputElement>('field-password').value = '';
  mustGet<HTMLTextAreaElement>('field-key').value = '';
  mustGet<HTMLInputElement>('field-passphrase').value = '';
  mustGet<HTMLInputElement>('field-clear-passphrase').checked = false;
  mustGet<HTMLInputElement>('field-clear-fp').checked = false;
  mustGet<HTMLElement>('form-error').textContent = '';
  const auth = host?.authType ?? 'password';
  const radio = form.querySelector<HTMLInputElement>(`input[name="auth"][value="${auth}"]`);
  if (radio) radio.checked = true;
  mustGet<HTMLElement>('secret-hint').textContent = host ? '密码或私钥留空表示不修改。' : '';
  const fp = mustGet<HTMLElement>('fingerprint-line');
  const clearFp = mustGet<HTMLElement>('clear-fp-row');
  if (host?.fingerprint) {
    fp.textContent = `已记住主机指纹 ${host.fingerprint}`;
    fp.classList.remove('hidden');
    clearFp.classList.remove('hidden');
  } else {
    fp.classList.add('hidden');
    clearFp.classList.add('hidden');
  }
  const clearPhrase = mustGet<HTMLElement>('clear-passphrase-row');
  clearPhrase.classList.toggle('hidden', !(host?.authType === 'privateKey' && host.hasPassphrase));
  syncAuthFields();
  dialog.showModal();
  mustGet<HTMLInputElement>('field-alias').focus();
}

async function saveForm(): Promise<void> {
  const authType = currentAuth();
  const draft: HostDraft = {
    id: editing?.id,
    alias: mustGet<HTMLInputElement>('field-alias').value,
    host: mustGet<HTMLInputElement>('field-host').value,
    port: Number(mustGet<HTMLInputElement>('field-port').value),
    username: mustGet<HTMLInputElement>('field-user').value,
    authType,
    secret: authType === 'password' ? mustGet<HTMLInputElement>('field-password').value : mustGet<HTMLTextAreaElement>('field-key').value,
    passphrase: mustGet<HTMLInputElement>('field-passphrase').value,
    clearPassphrase: mustGet<HTMLInputElement>('field-clear-passphrase').checked,
    clearFingerprint: mustGet<HTMLInputElement>('field-clear-fp').checked,
  };
  const result = await api.saveHost(draft);
  const error = mustGet<HTMLElement>('form-error');
  if (!result.ok) {
    error.textContent = result.error;
    return;
  }
  dialog.close();
  selectedId = result.data.id;
  await refresh();
  toast(editing ? '主机已更新' : '主机已保存');
}

async function removeHost(host: PublicHost): Promise<void> {
  mustGet<HTMLElement>('confirm-text').textContent = `删除主机「${host.alias}」？本机保存的登录信息会一起去掉。`;
  confirmDialog.showModal();
  const accepted = await new Promise<boolean>((resolve) => {
    confirmDialog.addEventListener('close', () => resolve(confirmDialog.returnValue === 'ok'), { once: true });
  });
  if (!accepted) return;
  if (sessionId && selectedId === host.id) await disconnect();
  const result = await api.deleteHost(host.id);
  if (!result.ok) {
    toast(result.error);
    return;
  }
  if (selectedId === host.id) selectedId = null;
  await refresh();
}

async function pickKey(): Promise<void> {
  const result = await api.pickPrivateKey();
  if (!result.ok) {
    mustGet<HTMLElement>('form-error').textContent = result.error;
    return;
  }
  if (!result.data) return;
  mustGet<HTMLTextAreaElement>('field-key').value = result.data.text;
  toast(`已读入 ${result.data.name}`);
}

function syncAuthFields(): void {
  const auth = currentAuth();
  mustGet('password-field').classList.toggle('hidden', auth !== 'password');
  mustGet('key-field').classList.toggle('hidden', auth !== 'privateKey');
}

function currentAuth(): AuthType {
  const selected = form.querySelector<HTMLInputElement>('input[name="auth"]:checked');
  return selected?.value === 'privateKey' ? 'privateKey' : 'password';
}

function paintStatus(): void {
  connectBtn.disabled = !selectedId || phase === 'connecting';
  disconnectBtn.disabled = phase !== 'connected';
  if (phase === 'idle') {
    const host = hosts.find((item) => item.id === selectedId);
    setPhase('idle', '未连接', host ? `${host.username}@${host.host}:${host.port}` : '从左边选一台主机');
  }
}

function setPhase(next: typeof phase, title: string, sub: string): void {
  phase = next;
  mustGet<HTMLElement>('status-title').textContent = title;
  mustGet<HTMLElement>('status-sub').textContent = sub;
  const dot = mustGet<HTMLElement>('status-dot');
  dot.className = `dot ${next === 'idle' ? '' : next}`.trim();
  connectBtn.disabled = !selectedId || next === 'connecting';
  disconnectBtn.disabled = next !== 'connected';
}

function smallButton(label: string, onClick: () => void): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = label;
  button.addEventListener('click', (event) => {
    event.stopPropagation();
    onClick();
  });
  return button;
}

function toast(message: string): void {
  const node = mustGet<HTMLElement>('toast');
  node.textContent = message;
  node.classList.add('show');
  window.setTimeout(() => node.classList.remove('show'), 4200);
}

function asBytes(data: Uint8Array | { data?: number[] }): Uint8Array {
  if (data instanceof Uint8Array) return data;
  if (data && Array.isArray(data.data)) return Uint8Array.from(data.data);
  return new Uint8Array();
}

function mustGet<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`缺少界面节点 ${id}`);
  return node as T;
}
