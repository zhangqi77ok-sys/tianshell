import { randomUUID } from 'node:crypto';
import type { AuthType, HostDraft, HostGroup, PublicHost, ResolvedHost } from '../shared/types.js';
import { atomicWriteJson, readJsonOrDefault } from './stores.js';
import { open, seal } from './vault.js';

interface StoredHost {
  id: string;
  alias: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  secret: string;
  passphrase: string | null;
  fingerprint: string | null;
  groupId?: string | null;
  jumpHostId?: string | null;
  tags?: string[];
  notes?: string;
  createdAt: string;
  updatedAt: string;
}

interface HostFile {
  version: 1;
  hosts: StoredHost[];
  groups?: HostGroup[];
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

  listGroups(): HostGroup[] {
    return [...(this.read().groups ?? [])].sort((a, b) => a.order - b.order || a.name.localeCompare(b.name, 'zh-CN'));
  }

  saveGroup(draft: { id?: string; name: string; parentId?: string | null; order?: number }): HostGroup {
    const name = requireText(draft.name, '分组名称', 48);
    const data = this.read();
    const groups = [...(data.groups ?? [])];
    const group: HostGroup = {
      id: draft.id?.trim() || randomUUID(),
      name,
      parentId: draft.parentId ?? null,
      order: typeof draft.order === 'number' ? draft.order : groups.length,
    };
    const idx = groups.findIndex((g) => g.id === group.id);
    if (idx >= 0) groups[idx] = group;
    else groups.push(group);
    atomicWriteJson(this.filePath, { version: 1, hosts: data.hosts, groups });
    return group;
  }

  deleteGroup(id: string): void {
    const data = this.read();
    const groups = (data.groups ?? []).filter((g) => g.id !== id);
    if (groups.length === (data.groups ?? []).length) throw new HostStoreError('找不到这个分组');
    const hosts = data.hosts.map((h) => (h.groupId === id ? { ...h, groupId: null } : h));
    atomicWriteJson(this.filePath, { version: 1, hosts, groups });
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
    assertNoJumpCycles(hosts);
    atomicWriteJson(this.filePath, { version: 1, hosts, groups: data.groups });
    return toPublic(next);
  }

  delete(id: string): void {
    const data = this.read();
    const hosts = data.hosts
      .filter((host) => host.id !== id)
      .map((host) => (host.jumpHostId === id ? { ...host, jumpHostId: null } : host));
    if (hosts.length === data.hosts.length) {
      throw new HostStoreError('找不到这台主机');
    }
    atomicWriteJson(this.filePath, { version: 1, hosts, groups: data.groups });
  }

  rememberFingerprint(id: string, fingerprint: string): boolean {
    const data = this.read();
    const host = data.hosts.find((item) => item.id === id);
    if (!host) return false;
    if (!host.fingerprint) {
      host.fingerprint = fingerprint;
      host.updatedAt = new Date().toISOString();
      atomicWriteJson(this.filePath, data);
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

    const secret = this.resolveSecret(draft, existing);
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
      groupId: draft.groupId === undefined ? (existing?.groupId ?? null) : draft.groupId,
      jumpHostId: draft.jumpHostId === undefined ? (existing?.jumpHostId ?? null) : draft.jumpHostId,
      tags: draft.tags ? draft.tags.map((t) => t.trim()).filter(Boolean) : existing?.tags,
      notes: draft.notes === undefined ? existing?.notes : draft.notes.trim(),
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
    };
  }

  private resolveSecret(draft: HostDraft, existing: StoredHost | undefined): string {
    const rawSecret = draft.secret ?? '';
    if (rawSecret.trim()) {
      if (draft.authType === 'privateKey' && !rawSecret.includes('PRIVATE KEY')) {
        throw new HostStoreError('私钥需要是 OpenSSH 或 PEM 格式（内容里要有 PRIVATE KEY）');
      }
      return seal(this.key, rawSecret);
    }
    if (existing && existing.authType === draft.authType) {
      return existing.secret;
    }
    throw new HostStoreError(draft.authType === 'password' ? '请填写密码' : '请填写私钥');
  }

  private read(): HostFile {
    let parsed: unknown;
    try {
      parsed = readJsonOrDefault<HostFile>(this.filePath, { version: 1, hosts: [] }, 'hosts.json');
    } catch {
      throw new HostStoreError('hosts.json 无法读取，已停止以免覆盖你的数据');
    }
    if (!isHostFile(parsed)) {
      throw new HostStoreError('hosts.json 格式无法识别，已停止以免覆盖你的数据');
    }
    return parsed;
  }
}

function assertNoJumpCycles(hosts: StoredHost[]): void {
  const byId = new Map(hosts.map((h) => [h.id, h]));
  for (const host of hosts) {
    const visited = new Set<string>([host.id]);
    let cur = host.jumpHostId ?? null;
    while (cur) {
      if (visited.has(cur)) throw new HostStoreError('跳板机引用存在循环依赖，请检查跳板机设置');
      visited.add(cur);
      cur = byId.get(cur)?.jumpHostId ?? null;
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
    groupId: host.groupId ?? null,
    jumpHostId: host.jumpHostId ?? null,
    tags: host.tags,
    notes: host.notes,
    createdAt: host.createdAt,
    updatedAt: host.updatedAt,
  };
}

function isHostFile(value: unknown): value is HostFile {
  if (!value || typeof value !== 'object') return false;
  const file = value as Partial<HostFile>;
  return file.version === 1 && Array.isArray(file.hosts);
}
