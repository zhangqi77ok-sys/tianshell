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

/**
 * 按键属于发出它的那个终端。
 * 这个终端已经断开时，结果是空的，不能再漏到其他被勾选的会话。
 */
export function keystrokeRecipients(
  sourceId: string,
  tabs: readonly BroadcastTab[],
  checked: readonly string[],
  enabled: boolean,
): string[] {
  const source = tabs.find((tab) => tab.id === sourceId);
  if (!source || source.closed) return [];
  return broadcastRecipients(sourceId, tabs, checked, enabled);
}

export type SessionOpenStep = 'focus' | 'reserve' | 'dispose' | 'connect';

/**
 * 真正要发起连接时，先占住这台主机的名额，再等待关闭旧标签。
 * 名额如果放在 await 之后，双击会在占位前再走进来一次。
 */
export function sessionOpenSteps(plan: SessionOpenPlan): SessionOpenStep[] {
  if (plan.wait) return [];
  if (plan.focusId) return ['focus'];
  const steps: SessionOpenStep[] = ['reserve'];
  if (plan.disposeId) steps.push('dispose');
  steps.push('connect');
  return steps;
}

/** 同一动作还在飞，就不要再接受一次点击。 */
export function claimSingleFlight(inFlight: boolean): boolean {
  return !inFlight;
}

/** 终端视图没显示，或槽位还藏着，量出来的格子不能信。 */
export function shouldMeasureTerminal(viewActive: boolean, slotHidden: boolean): boolean {
  return viewActive && !slotHidden;
}

/** 尺寸没变，或还没到能交给 PTY 的网格，就不要发 resize。 */
export function resizeIfChanged(
  previous: { cols: number; rows: number } | null,
  cols: number,
  rows: number,
): { cols: number; rows: number } | null {
  if (cols < 30 || rows < 6) return null;
  if (previous && previous.cols === cols && previous.rows === rows) return null;
  return { cols, rows };
}

export type EditorWrite = 'write' | 'busy' | 'disconnected' | 'idle';

/** 只写回打开编辑器时的那个仍连接会话。上一笔保存还没结束时不再叠一笔。 */
export function editorWriteGuard(
  editingSession: string | null,
  activeSession: string | null,
  inFlight: boolean,
): EditorWrite {
  if (!editingSession) return 'idle';
  if (inFlight) return 'busy';
  if (!activeSession || activeSession !== editingSession) return 'disconnected';
  return 'write';
}

/** 监控数字只画在仍连接、且正看着的那个会话上。 */
export function acceptLiveMetrics(sessionId: string, activeId: string | null, closed: boolean): boolean {
  return Boolean(activeId) && activeId === sessionId && !closed;
}

/** 后返回的历史或目录结果才算数。0 不是有效序号。 */
export function isCurrentGeneration(ticket: number, latest: number): boolean {
  return ticket > 0 && ticket === latest;
}

/** 空列表要分清「还没有命令」和「这次搜索没有匹配」。 */
export function historyEmptyText(query: string): string {
  const q = query.trim();
  if (!q) return '还没有命令历史。在终端里回车，或让 MCP 执行命令后会出现在这里。';
  return `没有匹配「${q}」的命令`;
}

/** 已经列出过历史时，刷新失败不能把那些命令换成一句说明。 */
export function historyAfterFailedRefresh(haveList: boolean): 'keep' | 'show-error' {
  return haveList ? 'keep' : 'show-error';
}

/** 清屏交给正在读键盘的 shell（和 Ctrl+L 一样），不在本地单独擦掉画面。 */
export function clearScreenInput(): string {
  return '\u000c';
}

/**
 * 快捷命令要单独执行。Ctrl+U 丢掉还没回车的半行，再送出命令和换行。
 * 命令汇编器同样把 Ctrl+U 当成清掉未提交输入。不发送 Ctrl+C，避免打断正在跑的程序。
 */
export function quickCommandInput(command: string): string {
  const text = command.replace(/[\r\n]+$/, '');
  if (!text.trim()) return '';
  return `\u0015${text}\n`;
}

export type InputDelivery = 'send' | 'no-terminal' | 'link-down';

/** 同一段断线里，输入失败只提示一次，避免每个按键都弹一条。 */
export const LINK_DOWN_NOTICE_GAP_MS = 4000;

/** 有接收终端且工作台连接开着才发送。连接中断和没有终端是两件不同的事。 */
export function inputDelivery(haveRecipients: boolean, linkOpen: boolean): InputDelivery {
  if (!haveRecipients) return 'no-terminal';
  if (!linkOpen) return 'link-down';
  return 'send';
}

export function shouldNotifyLinkDown(lastNotifiedAt: number, now: number, gapMs = LINK_DOWN_NOTICE_GAP_MS): boolean {
  if (lastNotifiedAt <= 0) return true;
  return now - lastNotifiedAt >= gapMs;
}

export type PasteDecision = 'send' | 'dismiss';

/** 多行粘贴只认第一次选择。取消或 Esc 之后，残留的提交不能再把内容送进终端。 */
export function settlePaste(alreadySettled: boolean, choice: PasteDecision): PasteDecision | 'ignore' {
  if (alreadySettled) return 'ignore';
  return choice;
}

/** 含换行且至少两行有内容时，粘贴会马上执行，需要先确认。 */
export function pasteNeedsConfirm(text: string): boolean {
  if (!text.includes('\n') && !text.includes('\r')) return false;
  return text.split(/\r?\n/).filter((line) => line.trim().length > 0).length > 1;
}

/** current < 0 表示还没定位。向前从第一条开始，向后从最后一条开始，到头之后回绕。 */
export function nextFindIndex(current: number, count: number, forward: boolean): number {
  if (count <= 0) return -1;
  if (current < 0) return forward ? 0 : count - 1;
  const step = forward ? 1 : -1;
  return (current + step + count) % count;
}
