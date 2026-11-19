import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createNodeSafeStorage } from '../src/core/dpapi.js';
import { loadOrCreateKey } from '../src/core/key-file.js';

test('pure-Node DPAPI SafeStorage seals TS1D vault.key on Windows and reads it back', () => {
  const safe = createNodeSafeStorage();
  if (process.platform !== 'win32') {
    assert.equal(safe.isEncryptionAvailable(), false);
    return;
  }

  assert.equal(safe.isEncryptionAvailable(), true);
  assert.equal(safe.magic, 'TS1D');

  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-dpapi-'));
  try {
    const file = path.join(dir, 'vault.key');
    const first = loadOrCreateKey(file, safe);
    const second = loadOrCreateKey(file, safe);
    assert.equal(first.length, 32);
    assert.deepEqual(first, second);
    const raw = readFileSync(file);
    assert.equal(raw.subarray(0, 4).toString('utf8'), 'TS1D');
    assert.equal(raw.includes(first), false);

    // Raw reader (safe = null) must reject TS1D file clearly
    assert.throws(() => loadOrCreateKey(file, null), /密钥环/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('pure-Node DPAPI reader explicitly rejects legacy Electron TS1S vault.key', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-legacy-'));
  try {
    const file = path.join(dir, 'vault.key');
    const legacySafe = {
      isEncryptionAvailable: () => true,
      encryptString: (plain: string) => Buffer.from(`enc:${plain}`, 'utf8'),
      decryptString: (buf: Buffer) => buf.toString('utf8').slice(4),
    };
    loadOrCreateKey(file, legacySafe);
    assert.equal(readFileSync(file).subarray(0, 4).toString('utf8'), 'TS1S');

    const nodeSafe = createNodeSafeStorage();
    if (nodeSafe.isEncryptionAvailable()) {
      assert.throws(() => loadOrCreateKey(file, nodeSafe), /Electron|TS1S/);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
