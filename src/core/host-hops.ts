import type { ResolvedHost } from '../shared/types.js';
import type { JumpHop } from '../ssh/client.js';
import type { HostStore } from './host-store.js';

export interface HostChain {
  target: ResolvedHost;
  hops: JumpHop[];
}

/**
 * Resolve target host and expand its ProxyJump chain from outermost bastion to target leaf.
 */
export function resolveHostChain(hosts: HostStore, idOrAlias: string): HostChain | null {
  const leaf = hosts.resolve(idOrAlias);
  if (!leaf) return null;

  const hops: JumpHop[] = [];
  const visited = new Set<string>();
  let current: ResolvedHost | null = leaf;

  while (current) {
    if (visited.has(current.id)) {
      throw new Error('跳板机链存在循环引用，请检查主机配置');
    }
    visited.add(current.id);
    const captured = current;
    hops.unshift({
      target: {
        host: captured.host,
        port: captured.port,
        username: captured.username,
        authType: captured.authType,
        secret: captured.secret,
        passphrase: captured.passphrase,
      },
      acceptHostKey: (fp) => hosts.rememberFingerprint(captured.id, fp),
    });
    if (!current.jumpHostId) break;
    const next = hosts.resolve(current.jumpHostId);
    if (!next) {
      throw new Error(`找不到主机「${current.alias}」引用的跳板机 (${current.jumpHostId})`);
    }
    current = next;
  }

  return { target: leaf, hops };
}
