import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import test from 'node:test';
import { open, seal } from '../src/core/vault.js';

test('seal then open returns the original secret', () => {
  const key = randomBytes(32);
  const sealed = seal(key, '正确的密码');
  assert.equal(open(key, sealed), '正确的密码');
  assert.equal(sealed.includes('正确的密码'), false);
});

test('a different key cannot open the secret', () => {
  const sealed = seal(randomBytes(32), 's3cret');
  assert.throws(() => open(randomBytes(32), sealed));
});

test('tampered ciphertext is rejected', () => {
  const key = randomBytes(32);
  const sealed = seal(key, 's3cret');
  const parts = sealed.split(':');
  const cipher = Buffer.from(parts[3], 'base64url');
  cipher[0] ^= 0xff;
  parts[3] = cipher.toString('base64url');
  assert.throws(() => open(key, parts.join(':')));
});
