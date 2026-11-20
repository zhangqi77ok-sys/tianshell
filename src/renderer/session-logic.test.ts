import assert from 'node:assert/strict';
import { test } from 'node:test';
import { broadcastRecipients, nextFindIndex, seedBroadcastChecks, type BroadcastTab } from './session-logic.ts';

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

test('查找下一条和上一条会回绕', () => {
  assert.equal(nextFindIndex(-1, 3, true), 0);
  assert.equal(nextFindIndex(-1, 3, false), 2);
  assert.equal(nextFindIndex(2, 3, true), 0);
  assert.equal(nextFindIndex(0, 3, false), 2);
  assert.equal(nextFindIndex(0, 0, true), -1);
});
