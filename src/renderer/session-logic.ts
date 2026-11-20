/** 广播和查找翻页的纯逻辑，方便在没有 DOM 的情况下核对。 */

export interface BroadcastTab {
  id: string;
  kind: 'ssh' | 'local';
  closed: boolean;
}

/** 打开广播时默认勾选仍连接的 SSH。本机终端和已断开的标签不勾。 */
export function seedBroadcastChecks(tabs: readonly BroadcastTab[]): string[] {
  return tabs.filter((tab) => !tab.closed && tab.kind === 'ssh').map((tab) => tab.id);
}

/**
 * 广播关闭时只把按键送给当前仍打开的终端。
 * 打开后，当前终端照常收到，另外只加勾选且仍打开的会话。
 */
export function broadcastRecipients(
  activeId: string | null,
  tabs: readonly BroadcastTab[],
  checked: readonly string[],
  enabled: boolean,
): string[] {
  const open = new Set(tabs.filter((tab) => !tab.closed).map((tab) => tab.id));
  const ids: string[] = [];
  if (activeId && open.has(activeId)) ids.push(activeId);
  if (!enabled) return ids;
  const seen = new Set(ids);
  for (const id of checked) {
    if (!open.has(id) || seen.has(id)) continue;
    seen.add(id);
    ids.push(id);
  }
  return ids;
}

export type SessionOpenIntent = 'reuse' | 'another';

export interface OpenTabRef {
  id: string;
  hostId: string | null;
  closed: boolean;
}

export interface SessionOpenPlan {
  /** 已有仍连接的会话时切过去。再开一个时为空。 */
  focusId: string | null;
  /** 复用时先关掉同一主机上已断开的标签。再开一个时不关。 */
  disposeId: string | null;
  /** 这台主机已经有一次连接在进行。 */
  wait: boolean;
}

/**
 * reuse：有活会话就切过去，否则清掉已断开的标签再开。
 * another：保留现有标签，另外再开一条 SSH。
 */
export function planSessionOpen(
  hostId: string,
  tabs: readonly OpenTabRef[],
  pending: boolean,
  intent: SessionOpenIntent,
): SessionOpenPlan {
  if (pending) return { focusId: null, disposeId: null, wait: true };
  if (intent === 'another') return { focusId: null, disposeId: null, wait: false };
  const live = tabs.find((tab) => tab.hostId === hostId && !tab.closed);
  if (live) return { focusId: live.id, disposeId: null, wait: false };
  const stale = tabs.find((tab) => tab.hostId === hostId && tab.closed);
  return { focusId: null, disposeId: stale?.id ?? null, wait: false };
}

/** 同一主机有多条会话时，按打开顺序加上 · 1、· 2，避免标签都叫同一个别名。 */
export function tabLabels(tabs: readonly { id: string; alias: string; hostId: string | null }[]): Map<string, string> {
  const totals = new Map<string, number>();
  for (const tab of tabs) {
    if (!tab.hostId) continue;
    totals.set(tab.hostId, (totals.get(tab.hostId) ?? 0) + 1);
  }
  const seen = new Map<string, number>();
  const labels = new Map<string, string>();
  for (const tab of tabs) {
    const total = tab.hostId ? totals.get(tab.hostId) ?? 1 : 1;
    if (!tab.hostId || total <= 1) {
      labels.set(tab.id, tab.alias);
      continue;
    }
    const index = (seen.get(tab.hostId) ?? 0) + 1;
    seen.set(tab.hostId, index);
    labels.set(tab.id, `${tab.alias} · ${index}`);
  }
  return labels;
}

/** current < 0 表示还没定位。向前从第一条开始，向后从最后一条开始，到头之后回绕。 */
export function nextFindIndex(current: number, count: number, forward: boolean): number {
  if (count <= 0) return -1;
  if (current < 0) return forward ? 0 : count - 1;
  const step = forward ? 1 : -1;
  return (current + step + count) % count;
}
