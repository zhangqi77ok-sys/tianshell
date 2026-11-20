import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  broadcastRecipients,
  nextFindIndex,
  planSessionOpen,
  seedBroadcastChecks,
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
