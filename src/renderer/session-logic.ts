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

/** current < 0 表示还没定位。向前从第一条开始，向后从最后一条开始，到头之后回绕。 */
export function nextFindIndex(current: number, count: number, forward: boolean): number {
  if (count <= 0) return -1;
  if (current < 0) return forward ? 0 : count - 1;
  const step = forward ? 1 : -1;
  return (current + step + count) % count;
}
