import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  acceptLiveMetrics,
  broadcastRecipients,
  claimSingleFlight,
  editorWriteGuard,
  clearScreenInput,
  historyAfterFailedRefresh,
  historyEmptyText,
  inputDelivery,
  isCurrentGeneration,
  LINK_DOWN_NOTICE_GAP_MS,
  shouldNotifyLinkDown,
  keystrokeRecipients,
  nextFindIndex,
  pasteNeedsConfirm,
  settlePaste,
  planSessionOpen,
  resizeIfChanged,
  seedBroadcastChecks,
  sessionOpenSteps,
  shouldMeasureTerminal,
  tabLabels,
  type BroadcastTab,
  type OpenTabRef,
} from './session-logic.ts';

const tabs: BroadcastTab[] = [
  { id: 'ssh-a', kind: 'ssh', closed: false },
  { id: 'ssh-b', kind: 'ssh', closed: false },
  { id: 'local', kind: 'local', closed: false },
  { id: 'dead', kind: 'ssh', closed: true },
];

test('广播默认勾选仍连接的 SSH，不勾本机和已断开的标签', () => {
  assert.deepEqual(seedBroadcastChecks(tabs), ['ssh-a', 'ssh-b']);
});

test('广播关闭时只进入当前仍打开的终端', () => {
  assert.deepEqual(broadcastRecipients('ssh-a', tabs, ['ssh-a', 'ssh-b', 'local'], false), ['ssh-a']);
  assert.deepEqual(broadcastRecipients('dead', tabs, ['ssh-b'], false), []);
  assert.deepEqual(broadcastRecipients(null, tabs, ['ssh-b'], false), []);
});

test('广播打开后只追加勾选且仍打开的会话', () => {
  assert.deepEqual(broadcastRecipients('local', tabs, ['ssh-a', 'dead', 'local'], true), ['local', 'ssh-a']);
  assert.deepEqual(broadcastRecipients('ssh-a', tabs, ['dead'], true), ['ssh-a']);
  assert.deepEqual(broadcastRecipients('ssh-a', tabs, ['ssh-b', 'local'], true), ['ssh-a', 'ssh-b', 'local']);
});

const hostTabs: OpenTabRef[] = [
  { id: 'live', hostId: 'h1', closed: false },
  { id: 'dead', hostId: 'h1', closed: true },
  { id: 'other', hostId: 'h2', closed: false },
];

test('已连接的主机再点一次是切回去，不替换标签', () => {
  assert.deepEqual(planSessionOpen('h1', hostTabs, false, 'reuse'), {
    focusId: 'live',
    disposeId: null,
    wait: false,
  });
});

test('再开一个会保留现有标签，包括已断开的', () => {
  assert.deepEqual(planSessionOpen('h1', hostTabs, false, 'another'), {
    focusId: null,
    disposeId: null,
    wait: false,
  });
});

test('没有活会话时，复用会先关掉已断开的标签', () => {
  assert.deepEqual(planSessionOpen('h1', [hostTabs[1]], false, 'reuse'), {
    focusId: null,
    disposeId: 'dead',
    wait: false,
  });
});

test('同一主机正在连接时不再开第二条', () => {
  assert.equal(planSessionOpen('h1', hostTabs, true, 'another').wait, true);
});

test('同一主机的多个标签按打开顺序区分', () => {
  const labels = tabLabels([
    { id: 'a', alias: '实验室', hostId: 'h1' },
    { id: 'b', alias: '本机', hostId: null },
    { id: 'c', alias: '实验室', hostId: 'h1' },
  ]);
  assert.equal(labels.get('a'), '实验室 · 1');
  assert.equal(labels.get('c'), '实验室 · 2');
  assert.equal(labels.get('b'), '本机');
});

test('查找下一条和上一条会回绕', () => {
  assert.equal(nextFindIndex(-1, 3, true), 0);
  assert.equal(nextFindIndex(-1, 3, false), 2);
  assert.equal(nextFindIndex(2, 3, true), 0);
  assert.equal(nextFindIndex(0, 3, false), 2);
  assert.equal(nextFindIndex(0, 0, true), -1);
});

test('按键进入发出它的终端，断开的终端不能漏到其他会话', () => {
  assert.deepEqual(keystrokeRecipients('ssh-b', tabs, [], false), ['ssh-b']);
  assert.deepEqual(keystrokeRecipients('ssh-a', tabs, ['ssh-b', 'local'], true), ['ssh-a', 'ssh-b', 'local']);
  assert.deepEqual(keystrokeRecipients('dead', tabs, ['ssh-a', 'ssh-b', 'local'], true), []);
  assert.deepEqual(keystrokeRecipients('missing', tabs, ['ssh-a'], true), []);
});

test('重连旧标签时先占住名额，再关闭旧标签', () => {
  const plan = planSessionOpen('h1', [{ id: 'dead', hostId: 'h1', closed: true }], false, 'reuse');
  const steps = sessionOpenSteps(plan);
  assert.deepEqual(steps, ['reserve', 'dispose', 'connect']);
  assert.ok(steps.indexOf('reserve') < steps.indexOf('dispose'));
  assert.deepEqual(sessionOpenSteps({ focusId: 'live', disposeId: null, wait: false }), ['focus']);
  assert.deepEqual(sessionOpenSteps({ focusId: null, disposeId: null, wait: true }), []);
});

test('重复点击和隐藏终端不会再发一次动作或尺寸', () => {
  assert.equal(claimSingleFlight(false), true);
  assert.equal(claimSingleFlight(true), false);
  assert.equal(shouldMeasureTerminal(true, false), true);
  assert.equal(shouldMeasureTerminal(false, false), false);
  assert.equal(shouldMeasureTerminal(true, true), false);
  assert.deepEqual(resizeIfChanged(null, 80, 24), { cols: 80, rows: 24 });
  assert.equal(resizeIfChanged({ cols: 80, rows: 24 }, 80, 24), null);
  assert.equal(resizeIfChanged(null, 12, 24), null);
  assert.equal(resizeIfChanged(null, 80, 4), null);
});

test('多行粘贴只执行第一次选择，取消后不能再提交', () => {
  assert.equal(settlePaste(false, 'send'), 'send');
  assert.equal(settlePaste(false, 'dismiss'), 'dismiss');
  assert.equal(settlePaste(true, 'send'), 'ignore');
  assert.equal(settlePaste(true, 'dismiss'), 'ignore');
  assert.equal(pasteNeedsConfirm('echo a\necho b\n'), true);
  assert.equal(pasteNeedsConfirm('echo a\r\necho b'), true);
  assert.equal(pasteNeedsConfirm('echo a\n'), false);
  assert.equal(pasteNeedsConfirm('echo a'), false);
});

test('清空屏幕送给 shell，而不是本地擦除', () => {
  const input = clearScreenInput();
  assert.equal(input, '\u000c');
  assert.equal(input.includes('\n'), false);
  assert.equal(input.includes('\u001b'), false);
});

test('历史搜索没有结果时，不说成还没有命令', () => {
  assert.match(historyEmptyText(''), /还没有命令历史/);
  assert.match(historyEmptyText('   '), /还没有命令历史/);
  assert.equal(historyEmptyText('zzzz'), '没有匹配「zzzz」的命令');
  assert.equal(historyAfterFailedRefresh(true), 'keep');
  assert.equal(historyAfterFailedRefresh(false), 'show-error');
});

test('工作台连接中断时输入不发出，并在间隔内只提示一次', () => {
  assert.equal(inputDelivery(true, true), 'send');
  assert.equal(inputDelivery(false, true), 'no-terminal');
  assert.equal(inputDelivery(false, false), 'no-terminal');
  assert.equal(inputDelivery(true, false), 'link-down');
  assert.equal(shouldNotifyLinkDown(0, 1), true);
  assert.equal(shouldNotifyLinkDown(1000, 1000 + LINK_DOWN_NOTICE_GAP_MS - 1), false);
  assert.equal(shouldNotifyLinkDown(1000, 1000 + LINK_DOWN_NOTICE_GAP_MS), true);
});

test('编辑器只写回打开它的会话，监控只采用仍有效的最新结果', () => {
  assert.equal(editorWriteGuard('s1', 's1', false), 'write');
  assert.equal(editorWriteGuard('s1', 's1', true), 'busy');
  assert.equal(editorWriteGuard('s1', null, false), 'disconnected');
  assert.equal(editorWriteGuard('s1', 's2', false), 'disconnected');
  assert.equal(editorWriteGuard(null, 's1', false), 'idle');
  assert.equal(acceptLiveMetrics('s1', 's1', false), true);
  assert.equal(acceptLiveMetrics('s1', 's2', false), false);
  assert.equal(acceptLiveMetrics('s1', 's1', true), false);
  assert.equal(isCurrentGeneration(2, 2), true);
  assert.equal(isCurrentGeneration(1, 2), false);
  assert.equal(isCurrentGeneration(0, 0), false);
});
