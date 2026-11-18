import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import {
  buildConnectConfig,
  explainSshError,
  fingerprintOf,
  HostKeyMismatchError,
  truncateOutput,
} from '../src/ssh/client.js';

const target = {
  host: '10.0.0.8',
  port: 22,
  username: 'ubuntu',
  authType: 'password' as const,
  secret: 'pw',
};

test('fingerprint matches an OpenSSH-style SHA256 digest', () => {
  const key = Buffer.from('host-key');
  const expected = `SHA256:${createHash('sha256').update(key).digest('base64').replace(/=+$/g, '')}`;
  assert.equal(fingerprintOf(key), expected);
});

test('connect config carries the password and asks the caller about the host key', () => {
  const seen: string[] = [];
  let allow = true;
  const built = buildConnectConfig(target, (fp) => {
    seen.push(fp);
    return allow;
  });
  assert.equal(built.config.password, 'pw');
  assert.equal(built.config.privateKey, undefined);
  const verifier = built.config.hostVerifier as (key: Buffer) => boolean;
  assert.equal(verifier(Buffer.from('good')), true);
  allow = false;
  assert.equal(verifier(Buffer.from('ok')), false);
  assert.equal(built.mismatch(), fingerprintOf(Buffer.from('ok')));
  assert.equal(seen[0], fingerprintOf(Buffer.from('good')));
});

test('private key auth puts the key material on the config', () => {
  const built = buildConnectConfig(
    {
      host: 'h',
      port: 2200,
      username: 'zq',
      authType: 'privateKey',
      secret: 'KEY',
      passphrase: 'phrase',
    },
    () => true,
  );
  assert.equal(built.config.privateKey, 'KEY');
  assert.equal(built.config.passphrase, 'phrase');
  assert.equal(built.config.port, 2200);
});

test('long command output is capped', () => {
  const full = truncateOutput('abc', 10);
  assert.deepEqual(full, { text: 'abc', truncated: false });
  const cut = truncateOutput('0123456789XYZ', 10);
  assert.equal(cut.truncated, true);
  assert.match(cut.text, /^0123456789/);
  assert.match(cut.text, /截断/);
});

test('ssh errors become short Chinese explanations', () => {
  assert.match(explainSshError(new Error('getaddrinfo ENOTFOUND box')), /找不到主机/);
  assert.match(explainSshError(new Error('connect ECONNREFUSED')), /拒绝/);
  assert.match(explainSshError(new Error('All configured authentication methods failed')), /认证失败/);
  assert.match(explainSshError(new HostKeyMismatchError('SHA256:abc')), /SHA256:abc/);
});
