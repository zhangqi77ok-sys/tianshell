import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { HostStore, HostStoreError } from '../src/core/host-store.js';
import { SettingsStore, SnippetStore, TunnelStore } from '../src/core/stores.js';

test('HostStore supports hierarchical groups, tags, notes, and jump-host cycle rejection', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-m1-'));
  try {
    const store = new HostStore(path.join(dir, 'hosts.json'), randomBytes(32));
    const grp = store.saveGroup({ name: '生产环境' });
    assert.equal(store.listGroups().length, 1);

    const bastion = store.save({
      alias: 'bastion',
      host: '1.2.3.4',
      username: 'jump',
      authType: 'password',
      secret: 'pw1',
      groupId: grp.id,
      tags: ['jump', 'prod'],
      notes: '出口跳板机',
    });
    const app = store.save({
      alias: 'app-01',
      host: '10.0.0.12',
      username: 'deploy',
      authType: 'password',
      secret: 'pw2',
      groupId: grp.id,
      jumpHostId: bastion.id,
    });

    assert.equal(app.groupId, grp.id);
    assert.equal(app.jumpHostId, bastion.id);

    // Self-loop or circular jump chain must be rejected cleanly
    assert.throws(
      () => store.save({ id: bastion.id, alias: 'bastion', host: '1.2.3.4', username: 'jump', authType: 'password', jumpHostId: bastion.id }),
      HostStoreError,
    );
    assert.throws(
      () => store.save({ id: bastion.id, alias: 'bastion', host: '1.2.3.4', username: 'jump', authType: 'password', jumpHostId: app.id }),
      HostStoreError,
    );

    // Deleting group unassigns member hosts without losing them
    store.deleteGroup(grp.id);
    assert.equal(store.listGroups().length, 0);
    assert.equal(store.resolve('app-01')?.groupId ?? null, null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('SnippetStore, TunnelStore, and SettingsStore persist CRUD safely', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-stores-'));
  try {
    const snippets = new SnippetStore(path.join(dir, 'snippets.json'));
    const s1 = snippets.save({ name: '查看端口', category: '网络', command: 'ss -tulnp' });
    assert.equal(snippets.list().length, 1);
    assert.equal(snippets.list()[0]?.id, s1.id);
    snippets.delete(s1.id);
    assert.equal(snippets.list().length, 0);

    const tunnels = new TunnelStore(path.join(dir, 'tunnels.json'));
    const t1 = tunnels.save({
      name: 'SOCKS5 代理',
      hostId: 'h1',
      mode: 'socks5',
      localHost: '127.0.0.1',
      localPort: 1080,
    });
    assert.equal(tunnels.list()[0]?.mode, 'socks5');
    assert.throws(
      () => tunnels.save({ name: '坏转发', hostId: 'h1', mode: 'local', localHost: '127.0.0.1', localPort: 70000 }),
      /端口/,
    );
    tunnels.delete(t1.id);
    assert.equal(tunnels.list().length, 0);

    const settings = new SettingsStore(path.join(dir, 'settings.json'));
    assert.equal(settings.get().theme, 'dark');
    const updated = settings.update({ theme: 'light', fontSize: 15, showHiddenFiles: true });
    assert.equal(updated.theme, 'light');
    assert.equal(updated.fontSize, 15);
    assert.equal(settings.get().showHiddenFiles, true);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
