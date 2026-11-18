import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { loadOrCreateKey, type SafeStorage } from '../src/core/key-file.js';

class MemorySafeStorage implements SafeStorage {
  available = true;
  isEncryptionAvailable(): boolean {
    return this.available;
  }
  encryptString(plain: string): Buffer {
    return Buffer.from(`enc:${plain}`, 'utf8');
  }
  decryptString(encrypted: Buffer): string {
    const text = encrypted.toString('utf8');
    assert.equal(text.startsWith('enc:'), true);
    return text.slice(4);
  }
}

test('creates a system-wrapped key and reads it back', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-key-'));
  try {
    const file = path.join(dir, 'vault.key');
    const safe = new MemorySafeStorage();
    const first = loadOrCreateKey(file, safe);
    const second = loadOrCreateKey(file, safe);
    assert.equal(first.length, 32);
    assert.deepEqual(first, second);
    assert.equal(readFileSync(file).subarray(0, 4).toString('utf8'), 'TS1S');
    assert.equal(readFileSync(file).includes(first), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('falls back to a user-only raw key when the system keystore is unavailable', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-key-'));
  try {
    const file = path.join(dir, 'vault.key');
    const safe = new MemorySafeStorage();
    safe.available = false;
    const key = loadOrCreateKey(file, safe);
    assert.equal(readFileSync(file).subarray(0, 4).toString('utf8'), 'TS1R');
    assert.deepEqual(loadOrCreateKey(file, null), key);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a raw-key reader refuses a system-wrapped key', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-key-'));
  try {
    const file = path.join(dir, 'vault.key');
    loadOrCreateKey(file, new MemorySafeStorage());
    assert.throws(() => loadOrCreateKey(file, null), /密钥环/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
