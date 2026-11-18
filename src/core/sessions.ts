import { randomUUID } from 'node:crypto';

export type SessionStatus = 'connecting' | 'running' | 'closed' | 'error';
export type SessionKind = 'shell' | 'exec';

export interface SessionInfo {
  id: string;
  hostId: string;
  alias: string;
  host: string;
  kind: SessionKind;
  status: SessionStatus;
  command?: string;
  startedAt: string;
  endedAt?: string;
  exitCode?: number | null;
  timedOut?: boolean;
  error?: string;
}

export class SessionRegistry {
  private items: SessionInfo[] = [];

  open(input: Omit<SessionInfo, 'id' | 'startedAt'>): SessionInfo {
    const info: SessionInfo = {
      ...input,
      id: randomUUID(),
      startedAt: new Date().toISOString(),
    };
    this.items.push(info);
    return info;
  }

  mark(id: string, patch: Partial<Omit<SessionInfo, 'id' | 'startedAt'>>): SessionInfo {
    const found = this.items.find((item) => item.id === id);
    if (!found) throw new Error('找不到会话');
    Object.assign(found, patch);
    return found;
  }

  get(id: string): SessionInfo | undefined {
    return this.items.find((item) => item.id === id);
  }

  list(): SessionInfo[] {
    return [...this.items].reverse();
  }
}
