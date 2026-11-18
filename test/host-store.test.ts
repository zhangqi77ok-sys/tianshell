import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { randomBytes } from 'node:crypto';
import { HostStore } from '../src/core/host-store.js';
import { open } from '../src/core/vault.js';

const KEY = '-----BEGIN OPENSSH PRIVATE KEY-----\nexample\n-----END OPENSSH PRIVATE KEY-----';

function tempStore(): { store: HostStore; dir: string; file: string; key: Buffer } {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-hosts-'));
  const key = randomBytes(32);
  const file = path.join(dir, 'hosts.json');
  return { store: new HostStore(file, key), dir, file, key };
}

test('saves a host without writing the password to disk', () => {
  const { store, dir, file, key } = tempStore();
  try {
    const saved = store.save({
      alias: '开发机',
      host: '10.0.0.8',
      username: 'ubuntu',
      authType: 'password',
      secret: '正确的密码',
    });
    assert.equal(saved.port, 22);
    assert.equal(saved.alias, '开发机');
    const listed = JSON.stringify(store.list());
    assert.equal(listed.includes('正确的密码'), false);
    assert.equal(listed.includes('secret'), false);
    const disk = readFileSync(file, 'utf8');
    assert.equal(disk.includes('正确的密码'), false);
    const resolved = store.resolve('开发机');
    assert.ok(resolved);
    assert.equal(resolved.secret, '正确的密码');
    const stored = JSON.parse(disk).hosts[0];
    assert.equal(open(key, stored.secret), '正确的密码');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('edits a host and keeps the secret when the password is left blank', () => {
  const { store, dir } = tempStore();
  try {
    const created = store.save({
      alias: 'dev',
      host: 'example.com',
      port: 2201,
      username: 'zq',
      authType: 'password',
      secret: 'old-pass',
    });
    const updated = store.save({
      id: created.id,
      alias: 'dev-2',
      host: 'example.com',
      port: 2201,
      username: 'zq',
      authType: 'password',
      secret: '   ',
    });
    assert.equal(updated.alias, 'dev-2');
    assert.equal(store.resolve(created.id)?.secret, 'old-pass');
    assert.equal(store.resolve('missing'), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('rejects a duplicate alias, a bad port, and a private key that is not a key', () => {
  const { store, dir } = tempStore();
  try {
    store.save({
      alias: 'Dev',
      host: '10.0.0.1',
      username: 'root',
      authType: 'password',
      secret: 'x',
    });
    assert.throws(
      () =>
        store.save({
          alias: 'dev',
          host: '10.0.0.2',
          username: 'root',
          authType: 'password',
          secret: 'y',
        }),
      /别名/,
    );
    assert.throws(
      () =>
        store.save({
          alias: 'other',
          host: '10.0.0.2',
          port: 0,
          username: 'root',
          authType: 'password',
          secret: 'y',
        }),
      /端口/,
    );
    assert.throws(
      () =>
        store.save({
          alias: 'key',
          host: '10.0.0.3',
          username: 'root',
          authType: 'privateKey',
          secret: 'not-a-key',
        }),
      /私钥/,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('stores a private key and passphrase sealed, and can clear the fingerprint', () => {
  const { store, dir, file } = tempStore();
  try {
    const saved = store.save({
      alias: 'jump',
      host: 'jump.example',
      username: 'zq',
      authType: 'privateKey',
      secret: KEY,
      passphrase: 'key-pass',
    });
    assert.equal(saved.hasPassphrase, true);
    assert.equal(readFileSync(file, 'utf8').includes('key-pass'), false);
    assert.equal(readFileSync(file, 'utf8').includes('BEGIN OPENSSH'), false);
    assert.equal(store.resolve('jump')?.passphrase, 'key-pass');

    assert.equal(store.rememberFingerprint(saved.id, 'SHA256:first'), true);
    assert.equal(store.resolve(saved.id)?.fingerprint, 'SHA256:first');
    assert.equal(store.rememberFingerprint(saved.id, 'SHA256:first'), true);
    assert.equal(store.rememberFingerprint(saved.id, 'SHA256:other'), false);
    assert.equal(store.resolve(saved.id)?.fingerprint, 'SHA256:first');

    store.save({
      id: saved.id,
      alias: 'jump',
      host: 'jump.example',
      username: 'zq',
      authType: 'privateKey',
      clearFingerprint: true,
      clearPassphrase: true,
    });
    const again = store.resolve('jump');
    assert.equal(again?.fingerprint, null);
    assert.equal(again?.passphrase, undefined);
    assert.equal(again?.secret, KEY);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('deletes a host and refuses to overwrite a corrupt file', () => {
  const { store, dir, file } = tempStore();
  try {
    const saved = store.save({
      alias: 'gone',
      host: '10.1.1.1',
      username: 'ubuntu',
      authType: 'password',
      secret: 'z',
    });
    store.delete(saved.id);
    assert.deepEqual(store.list(), []);
    writeCorrupt(file);
    assert.throws(() => store.list(), /hosts\.json/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

function writeCorrupt(file: string): void {
  writeFileSync(file, '{', 'utf8');
}
