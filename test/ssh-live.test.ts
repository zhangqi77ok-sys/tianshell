import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { execCommand, explainSshError, openShell, type SshTarget } from '../src/ssh/client.js';

const require = createRequire(import.meta.url);
const { Server } = require('ssh2') as typeof import('ssh2');

function hostKey(): string {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-hostkey-'));
  const file = path.join(dir, 'host');
  execFileSync('ssh-keygen', ['-t', 'ed25519', '-f', file, '-N', '', '-q']);
  const key = readFileSync(file, 'utf8');
  rmSync(dir, { recursive: true, force: true });
  return key;
}

async function listen(): Promise<{ port: number; close: () => Promise<void> }> {
  const server = new Server({ hostKeys: [hostKey()] }, (client) => {
    client.on('authentication', (ctx) => {
      if (ctx.method === 'password' && ctx.username === 'ubuntu' && ctx.password === 'secret-pass') ctx.accept();
      else ctx.reject();
    });
    client.on('ready', () => {
      client.on('session', (accept) => {
        const session = accept();
        session.on('pty', (acceptPty) => acceptPty());
        session.on('window-change', (acceptWindow) => acceptWindow());
        session.on('exec', (acceptExec, _reject, info) => {
          const stream = acceptExec();
          if (info.command === 'hang') return;
          stream.write(`ran:${info.command}`);
          stream.exit(0);
          stream.close();
        });
        session.on('shell', (acceptShell) => {
          const stream = acceptShell();
          stream.write('ready\n');
          stream.on('data', (chunk: Buffer) => {
            if (chunk.toString('utf8').includes('ping')) stream.write('pong\n');
          });
        });
      });
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('测试 SSH 服务没有端口');
  return {
    port: address.port,
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
      }),
  };
}

function target(port: number, secret = 'secret-pass'): SshTarget {
  return { host: '127.0.0.1', port, username: 'ubuntu', authType: 'password', secret };
}

async function until(predicate: () => boolean): Promise<void> {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > 3000) throw new Error('等待 SSH 输出超时');
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test('executes a command on a real local SSH server', { timeout: 8000 }, async () => {
  const server = await listen();
  try {
    const result = await execCommand(target(server.port), 'uname', 3000, () => true);
    assert.equal(result.timedOut, false);
    assert.equal(result.exitCode, 0);
    assert.equal(result.stdout, 'ran:uname');
  } finally {
    await server.close();
  }
});

test('opens an interactive shell and round-trips input', { timeout: 8000 }, async () => {
  const server = await listen();
  try {
    let output = '';
    const shell = await openShell({
      target: target(server.port),
      cols: 80,
      rows: 24,
      acceptHostKey: () => true,
      onData: (chunk) => {
        output += chunk.toString('utf8');
      },
      onClose: () => undefined,
    });
    await until(() => output.includes('ready'));
    shell.write('ping\n');
    await until(() => output.includes('pong'));
    shell.close();
  } finally {
    await server.close();
  }
});

test('rejects a bad password and times out a stuck command', { timeout: 8000 }, async () => {
  const server = await listen();
  try {
    await assert.rejects(execCommand(target(server.port, 'nope'), 'uname', 3000, () => true), (error: unknown) => {
      assert.match(explainSshError(error), /认证失败/);
      return true;
    });
    const hung = await execCommand(target(server.port), 'hang', 400, () => true);
    assert.equal(hung.timedOut, true);
    assert.equal(hung.exitCode, null);
  } finally {
    await server.close();
  }
});
