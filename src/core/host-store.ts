import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import type { HostDraft, PublicHost, ResolvedHost } from '../shared/types.js';
import { open, seal } from './vault.js';

interface StoredHost {
  id: string;
  alias: string;
  host: string;
  port: number;
  username: string;
  authType: 'password' | 'privateKey';
  secret: string;
  passphrase: string | null;
  fingerprint: string | null;
  createdAt: string;
  updatedAt: string;
}

interface HostFile {
  version: 1;
  hosts: StoredHost[];
}

export class HostStoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'HostStoreError';
  }
}

export class HostStore {
  constructor(
    private readonly filePath: string,
    private readonly key: Buffer,
  ) {}

  list(): PublicHost[] {
    return this.read()
      .hosts.map(toPublic)
      .sort((a, b) => a.alias.localeCompare(b.alias, 'zh-CN'));
  }

  resolve(idOrAlias: string): ResolvedHost | null {
    const needle = idOrAlias.trim();
    const found = this.read().hosts.find(
      (host) => host.id === needle || host.alias.toLowerCase() === needle.toLowerCase(),
    );
    if (!found) return null;
    return {
      ...toPublic(found),
      secret: open(this.key, found.secret),
      passphrase: found.passphrase ? open(this.key, found.passphrase) : undefined,
    };
  }

  save(draft: HostDraft): PublicHost {
    const data = this.read();
    const existing = draft.id ? data.hosts.find((host) => host.id === draft.id) : undefined;
    if (draft.id && !existing) {
      throw new HostStoreError('找不到要编辑的主机');
    }
    const next = this.normalize(draft, existing);
    const aliasTaken = data.hosts.some(
      (host) => host.id !== next.id && host.alias.toLowerCase() === next.alias.toLowerCase(),
    );
    if (aliasTaken) {
      throw new HostStoreError(`已经有别名叫「${next.alias}」的主机了`);
    }
    const hosts = existing ? data.hosts.map((host) => (host.id === next.id ? next : host)) : [...data.hosts, next];
    this.write({ version: 1, hosts });
    return toPublic(next);
  }

  delete(id: string): void {
    const data = this.read();
    const hosts = data.hosts.filter((host) => host.id !== id);
    if (hosts.length === data.hosts.length) {
      throw new HostStoreError('找不到这台主机');
    }
    this.write({ version: 1, hosts });
  }

  rememberFingerprint(id: string, fingerprint: string): boolean {
    const data = this.read();
    const host = data.hosts.find((item) => item.id === id);
    if (!host) return false;
    if (!host.fingerprint) {
      host.fingerprint = fingerprint;
      host.updatedAt = new Date().toISOString();
      this.write(data);
      return true;
    }
    return host.fingerprint === fingerprint;
  }

  private normalize(draft: HostDraft, existing: StoredHost | undefined): StoredHost {
    const alias = requireText(draft.alias, '别名', 64);
    const host = requireText(draft.host, '主机地址', 253);
    if (/\s/.test(host)) throw new HostStoreError('主机地址里不能有空格');
    const username = requireText(draft.username, '用户名', 128);
    if (/\s/.test(username)) throw new HostStoreError('用户名里不能有空格');
    const port = draft.port === undefined ? 22 : draft.port;
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new HostStoreError('端口需要是 1 到 65535 的整数');
    }
    if (draft.authType !== 'password' && draft.authType !== 'privateKey') {
      throw new HostStoreError('认证方式只能是密码或私钥');
    }

    const rawSecret = draft.secret ?? '';
    let secret: string;
    if (rawSecret.trim()) {
      if (draft.authType === 'privateKey' && !rawSecret.includes('PRIVATE KEY')) {
        throw new HostStoreError('私钥需要是 OpenSSH 或 PEM 格式（内容里要有 PRIVATE KEY）');
      }
      secret = seal(this.key, rawSecret);
    } else if (existing && existing.authType === draft.authType) {
      secret = existing.secret;
    } else {
      throw new HostStoreError(draft.authType === 'password' ? '请填写密码' : '请填写私钥');
    }

    let passphrase: string | null = null;
    if (draft.authType === 'privateKey') {
      if (draft.clearPassphrase) passphrase = null;
      else if (draft.passphrase) passphrase = seal(this.key, draft.passphrase);
      else passphrase = existing?.passphrase ?? null;
    }

    const now = new Date().toISOString();
    return {
      id: existing?.id ?? randomUUID(),
      alias,
      host,
      port,
      username,
      authType: draft.authType,
      secret,
      passphrase,
      fingerprint: draft.clearFingerprint ? null : (existing?.fingerprint ?? null),
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
    };
  }

  private read(): HostFile {
    if (!existsSync(this.filePath)) return { version: 1, hosts: [] };
    let parsed: unknown;
    try {
      parsed = JSON.parse(readFileSync(this.filePath, 'utf8'));
    } catch {
      throw new HostStoreError('hosts.json 无法读取，已停止以免覆盖你的数据');
    }
    if (!isHostFile(parsed)) {
      throw new HostStoreError('hosts.json 格式无法识别，已停止以免覆盖你的数据');
    }
    return parsed;
  }

  private write(data: HostFile): void {
    mkdirSync(path.dirname(this.filePath), { recursive: true, mode: 0o700 });
    const tmp = `${this.filePath}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(data, null, 2), { mode: 0o600 });
    renameSync(tmp, this.filePath);
    try {
      chmodSync(this.filePath, 0o600);
    } catch {
      // 同上，Windows 的权限模型和 chmod 不完全对应。
    }
  }
}

function requireText(value: string, label: string, max: number): string {
  const text = value?.trim() ?? '';
  if (!text) throw new HostStoreError(`请填写${label}`);
  if (text.length > max) throw new HostStoreError(`${label}太长了`);
  return text;
}

function toPublic(host: StoredHost): PublicHost {
  return {
    id: host.id,
    alias: host.alias,
    host: host.host,
    port: host.port,
    username: host.username,
    authType: host.authType,
    hasPassphrase: Boolean(host.passphrase),
    fingerprint: host.fingerprint,
    createdAt: host.createdAt,
    updatedAt: host.updatedAt,
  };
}

function isHostFile(value: unknown): value is HostFile {
  if (!value || typeof value !== 'object') return false;
  const file = value as Partial<HostFile>;
  return file.version === 1 && Array.isArray(file.hosts);
}
