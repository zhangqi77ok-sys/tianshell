import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  deletePrompt,
  editorSaveBlock,
  failedLoadEffect,
  keepEditorOpen,
  chmodPrompt,
  parseOctalMode,
  planUploads,
  remoteChildPath,
} from './sftp-logic.ts';

test('删除符号链接不会说成递归删除目录', () => {
  const link = deletePrompt('symlink', 'link');
  assert.match(link.title, /链接/);
  assert.match(link.body, /不会删除它指向的/);
  const dir = deletePrompt('dir', 'logs');
  assert.match(dir.body, /递归删除/);
  const file = deletePrompt('file', 'a.txt');
  assert.match(file.body, /文件会从远端删除/);
});

test('符号链接的权限框说明改的是目标', () => {
  assert.match(chmodPrompt('symlink'), /指向的文件/);
  assert.match(chmodPrompt('symlink'), /不是链接本身/);
  assert.doesNotMatch(chmodPrompt('file'), /链接/);
  assert.match(chmodPrompt('dir'), /八进制/);
});

test('只预览了前一段的文件不能写回', () => {
  assert.equal(editorSaveBlock(false), null);
  assert.match(editorSaveBlock(true) ?? '', /截掉/);
});

test('目录跳转失败时留下已经列出的文件', () => {
  assert.equal(failedLoadEffect(true), 'keep');
  assert.equal(failedLoadEffect(false), 'show-error');
});

test('未确认放弃时，编辑器保持打开', () => {
  assert.equal(keepEditorOpen(true, false), true);
  assert.equal(keepEditorOpen(true, true), false);
  assert.equal(keepEditorOpen(false, false), false);
});

test('上传路径停在当前目录里', () => {
  assert.equal(remoteChildPath('/var/log', 'app.log'), '/var/log/app.log');
  assert.equal(remoteChildPath('/', 'a.txt'), '/a.txt');
  assert.equal(remoteChildPath('/tmp/', 'a.txt'), '/tmp/a.txt');
  assert.equal(remoteChildPath('/tmp', '../etc/passwd'), null);
  assert.equal(remoteChildPath('/tmp', 'a/b'), null);
  assert.equal(remoteChildPath('/tmp', '.'), null);
  assert.equal(remoteChildPath('/tmp', ''), null);
});

test('上传计划拒绝超限和坏名字，并指出会覆盖的文件', () => {
  const plan = planUploads('/data', ['keep.txt', 'old.txt'], [
    { name: 'new.txt', size: 12 },
    { name: 'old.txt', size: 4 },
    { name: 'big.bin', size: 64 * 1024 * 1024 + 1 },
    { name: '../x', size: 1 },
    { name: 'new.txt', size: 3 },
  ]);
  assert.deepEqual(plan.send.map((item) => item.path), ['/data/new.txt', '/data/old.txt']);
  assert.deepEqual(plan.overwrite, ['old.txt']);
  assert.equal(plan.skipped.length, 3);
  assert.match(plan.skipped[0].reason, /64MB/);
  assert.match(plan.skipped[1].reason, /斜杠/);
  assert.match(plan.skipped[2].reason, /重名/);
});

test('权限只接受三位或四位八进制', () => {
  assert.equal(parseOctalMode('755'), 0o755);
  assert.equal(parseOctalMode('0644'), 0o644);
  assert.equal(parseOctalMode('7555'), 0o7555);
  assert.equal(parseOctalMode('000'), 0);
  assert.equal(parseOctalMode('999'), null);
  assert.equal(parseOctalMode('75'), null);
  assert.equal(parseOctalMode(''), null);
  assert.equal(parseOctalMode('75555'), null);
});
