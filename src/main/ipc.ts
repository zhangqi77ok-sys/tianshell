import { dialog, ipcMain, type BrowserWindow } from 'electron';
import { readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import type { HostStore } from '../core/host-store.js';
import type { SessionRegistry } from '../core/sessions.js';
import type { ConnectInfo, HostDraft, IpcResult, PrivateKeyPick } from '../shared/types.js';
import { explainSshError, openShell, type ShellSession, type SshTarget } from '../ssh/client.js';

interface IpcDeps {
  dataDir: string;
  hosts: HostStore;
  sessions: SessionRegistry;
  shells: Map<string, ShellSession>;
  getWindow: () => BrowserWindow | null;
}

export function registerIpc(deps: IpcDeps): void {
  ipcMain.handle('hosts:list', () => guard(() => deps.hosts.list()));
  ipcMain.handle('hosts:save', (_event, draft: unknown) => guard(() => deps.hosts.save(asDraft(draft))));
  ipcMain.handle('hosts:delete', (_event, id: unknown) =>
    guard(() => {
      if (typeof id !== 'string' || !id) throw new Error('主机编号不对');
      deps.hosts.delete(id);
      return { id };
    }),
  );
  ipcMain.handle('dialog:privateKey', () => pickPrivateKey(deps.getWindow()));
  ipcMain.handle('app:dataDir', () => guard(() => deps.dataDir));
  ipcMain.handle('ssh:connect', (_event, hostId: unknown, cols: unknown, rows: unknown) =>
    connectHost(deps, hostId, cols, rows),
  );
  ipcMain.handle('ssh:disconnect', (_event, sessionId: unknown) =>
    guard(() => {
      if (typeof sessionId !== 'string') throw new Error('会话编号不对');
      deps.shells.get(sessionId)?.close();
      return { sessionId };
    }),
  );
  ipcMain.on('ssh:write', (_event, sessionId: unknown, data: unknown) => {
    if (typeof sessionId !== 'string' || typeof data !== 'string' || data.length > 200_000) return;
    deps.shells.get(sessionId)?.write(data);
  });
  ipcMain.on('ssh:resize', (_event, sessionId: unknown, cols: unknown, rows: unknown) => {
    if (typeof sessionId !== 'string' || typeof cols !== 'number' || typeof rows !== 'number') return;
    deps.shells.get(sessionId)?.resize(cols, rows);
  });
}

function guard<T>(fn: () => T): IpcResult<T> {
  try {
    return { ok: true, data: fn() };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : '出了点问题' };
  }
}

function asDraft(value: unknown): HostDraft {
  if (!value || typeof value !== 'object') throw new Error('主机信息不完整');
  const draft = value as Partial<HostDraft>;
  if (typeof draft.alias !== 'string' || typeof draft.host !== 'string' || typeof draft.username !== 'string') {
    throw new Error('主机信息不完整');
  }
  if (draft.authType !== 'password' && draft.authType !== 'privateKey') throw new Error('认证方式不对');
  if (draft.port !== undefined && typeof draft.port !== 'number') throw new Error('端口不对');
  if (draft.secret !== undefined && typeof draft.secret !== 'string') throw new Error('凭据不对');
  if (draft.passphrase !== undefined && typeof draft.passphrase !== 'string') throw new Error('口令不对');
  if (draft.id !== undefined && typeof draft.id !== 'string') throw new Error('主机编号不对');
  return {
    id: draft.id,
    alias: draft.alias,
    host: draft.host,
    port: draft.port,
    username: draft.username,
    authType: draft.authType,
    secret: draft.secret,
    passphrase: draft.passphrase,
    clearPassphrase: Boolean(draft.clearPassphrase),
    clearFingerprint: Boolean(draft.clearFingerprint),
  };
}

async function pickPrivateKey(win: BrowserWindow | null): Promise<IpcResult<PrivateKeyPick | null>> {
  try {
    const options = { title: '选择私钥', properties: ['openFile'] as Array<'openFile'> };
    const result = win ? await dialog.showOpenDialog(win, options) : await dialog.showOpenDialog(options);
    if (result.canceled || !result.filePaths[0]) return { ok: true, data: null };
    const filePath = result.filePaths[0];
    if (statSync(filePath).size > 256 * 1024) return { ok: false, error: '这个文件太大了，不太像私钥' };
    return { ok: true, data: { name: path.basename(filePath), text: readFileSync(filePath, 'utf8') } };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : '读不到这个私钥文件' };
  }
}

async function connectHost(
  deps: IpcDeps,
  hostId: unknown,
  cols: unknown,
  rows: unknown,
): Promise<IpcResult<ConnectInfo>> {
  if (typeof hostId !== 'string' || !hostId) return { ok: false, error: '请先选一台主机' };
  const resolved = deps.hosts.resolve(hostId);
  if (!resolved) return { ok: false, error: '找不到这台主机' };

  for (const shell of deps.shells.values()) shell.close();

  const session = deps.sessions.open({
    hostId: resolved.id,
    alias: resolved.alias,
    host: resolved.host,
    kind: 'shell',
    status: 'connecting',
  });
  let closed = false;
  const noteClosed = () => {
    if (closed) return;
    closed = true;
    deps.shells.delete(session.id);
    const current = deps.sessions.get(session.id);
    if (current && current.status !== 'error') {
      deps.sessions.mark(session.id, { status: 'closed', endedAt: new Date().toISOString() });
    }
    const win = deps.getWindow();
    if (win && !win.isDestroyed()) win.webContents.send('ssh:closed', session.id);
  };

  const target: SshTarget = {
    host: resolved.host,
    port: resolved.port,
    username: resolved.username,
    authType: resolved.authType,
    secret: resolved.secret,
    passphrase: resolved.passphrase,
  };

  try {
    const shell = await openShell({
      target,
      cols: typeof cols === 'number' ? cols : 80,
      rows: typeof rows === 'number' ? rows : 24,
      acceptHostKey: (fingerprint) => deps.hosts.rememberFingerprint(resolved.id, fingerprint),
      onData: (chunk) => {
        const win = deps.getWindow();
        if (!win || win.isDestroyed()) return;
        win.webContents.send('ssh:data', session.id, new Uint8Array(chunk));
      },
      onClose: noteClosed,
    });
    deps.shells.set(session.id, shell);
    deps.sessions.mark(session.id, { status: 'running' });
    return {
      ok: true,
      data: {
        sessionId: session.id,
        alias: resolved.alias,
        username: resolved.username,
        host: resolved.host,
        port: resolved.port,
      },
    };
  } catch (error) {
    const message = explainSshError(error);
    deps.sessions.mark(session.id, { status: 'error', error: message, endedAt: new Date().toISOString() });
    closed = true;
    return { ok: false, error: message };
  }
}
