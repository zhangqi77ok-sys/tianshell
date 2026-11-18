import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import type { AppSettings, CommandSnippet, TunnelMode, TunnelRule } from '../shared/types.js';

const RETRY_CODES = new Set(['EPERM', 'EBUSY', 'EACCES']);

/**
 * Atomically write JSON with Windows EPERM/EBUSY retry protection to prevent partial corruption.
 */
export function atomicWriteJson(filePath: string, data: unknown): void {
  mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  const tmp = `${filePath}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(tmp, JSON.stringify(data, null, 2), { mode: 0o600 });

  let lastErr: unknown = null;
  for (let attempt = 0; attempt < 6; attempt += 1) {
    try {
      renameSync(tmp, filePath);
      lastErr = null;
      break;
    } catch (err) {
      lastErr = err;
      const code = (err as NodeJS.ErrnoException)?.code ?? '';
      if (!RETRY_CODES.has(code) || attempt === 5) break;
      const waitUntil = Date.now() + 20 * (attempt + 1);
      while (Date.now() < waitUntil) {
        // Short synchronous backoff prevents cross-process rename collisions on Windows.
      }
    }
  }
  if (lastErr) {
    try {
      rmSync(tmp, { force: true });
    } catch {
      // Ignore temp cleanup failures when surfacing the root rename error.
    }
    throw lastErr;
  }
  try {
    chmodSync(filePath, 0o600);
  } catch {
    // Windows relies on ACL/DPAPI rather than POSIX chmod bits.
  }
}

/**
 * Safely parse JSON file or return fallback when missing; throws explicitly on corrupt JSON.
 */
export function readJsonOrDefault<T>(filePath: string, fallback: T, label: string): T {
  if (!existsSync(filePath)) return fallback;
  try {
    return JSON.parse(readFileSync(filePath, 'utf8')) as T;
  } catch {
    throw new Error(`${label} 无法读取，已停止以免覆盖数据`);
  }
}

function validatePort(port: number, label: string): number {
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`${label}端口必须是 1 到 65535 的整数`);
  }
  return port;
}

export class SnippetStore {
  constructor(private readonly filePath: string) {}

  list(): CommandSnippet[] {
    const items = readJsonOrDefault<CommandSnippet[]>(this.filePath, [], 'snippets.json');
    return Array.isArray(items) ? items : [];
  }

  save(draft: Omit<CommandSnippet, 'id'> & { id?: string }): CommandSnippet {
    const name = draft.name?.trim() ?? '';
    const command = draft.command?.trim() ?? '';
    if (!name) throw new Error('请填写快捷命令名称');
    if (!command) throw new Error('请填写快捷命令内容');
    const items = this.list();
    const item: CommandSnippet = {
      id: draft.id?.trim() || randomUUID(),
      name,
      category: draft.category?.trim() || '常用',
      command,
      description: draft.description?.trim() || undefined,
    };
    const exists = items.some((x) => x.id === item.id);
    const next = exists ? items.map((x) => (x.id === item.id ? item : x)) : [...items, item];
    atomicWriteJson(this.filePath, next);
    return item;
  }

  delete(id: string): void {
    const items = this.list();
    const next = items.filter((x) => x.id !== id);
    if (next.length === items.length) throw new Error('找不到该快捷命令');
    atomicWriteJson(this.filePath, next);
  }
}

export class TunnelStore {
  constructor(private readonly filePath: string) {}

  list(): TunnelRule[] {
    const items = readJsonOrDefault<TunnelRule[]>(this.filePath, [], 'tunnels.json');
    return Array.isArray(items) ? items : [];
  }

  save(draft: Omit<TunnelRule, 'id'> & { id?: string }): TunnelRule {
    const name = draft.name?.trim() ?? '';
    if (!name) throw new Error('请填写隧道名称');
    if (!draft.hostId?.trim()) throw new Error('请选择关联 SSH 主机');
    const mode: TunnelMode = draft.mode;
    if (mode !== 'local' && mode !== 'remote' && mode !== 'socks5') {
      throw new Error('不支持的隧道模式');
    }
    const localPort = validatePort(draft.localPort, '本地');
    const remoteHost = draft.remoteHost?.trim() || '127.0.0.1';
    const remotePort = mode === 'socks5' ? undefined : validatePort(draft.remotePort ?? 0, '目标');
    const rule: TunnelRule = {
      id: draft.id?.trim() || randomUUID(),
      name,
      hostId: draft.hostId.trim(),
      mode,
      localHost: draft.localHost?.trim() || '127.0.0.1',
      localPort,
      remoteHost: mode === 'socks5' ? undefined : remoteHost,
      remotePort,
    };
    const items = this.list();
    const exists = items.some((x) => x.id === rule.id);
    const next = exists ? items.map((x) => (x.id === rule.id ? rule : x)) : [...items, rule];
    atomicWriteJson(this.filePath, next);
    return rule;
  }

  delete(id: string): void {
    const items = this.list();
    const next = items.filter((x) => x.id !== id);
    if (next.length === items.length) throw new Error('找不到该隧道规则');
    atomicWriteJson(this.filePath, next);
  }
}

export class SettingsStore {
  constructor(private readonly filePath: string) {}

  get(): AppSettings {
    return this.normalize(readJsonOrDefault<Partial<AppSettings>>(this.filePath, {}, 'settings.json'));
  }

  update(patch: Partial<AppSettings>): AppSettings {
    const merged = this.normalize({ ...this.get(), ...patch });
    atomicWriteJson(this.filePath, merged);
    return merged;
  }

  private normalize(raw: Partial<AppSettings>): AppSettings {
    return {
      theme: raw.theme === 'light' ? 'light' : 'dark',
      fontSize: typeof raw.fontSize === 'number' && raw.fontSize >= 11 && raw.fontSize <= 22 ? raw.fontSize : 14,
      showHiddenFiles: Boolean(raw.showHiddenFiles),
      monitorIntervalMs:
        typeof raw.monitorIntervalMs === 'number' && raw.monitorIntervalMs >= 1000 && raw.monitorIntervalMs <= 10000
          ? raw.monitorIntervalMs
          : 2500,
    };
  }
}
