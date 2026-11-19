import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import net from 'node:net';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { connectWithJumps, type JumpHop } from '../src/ssh/client.js';
import { createCrLfNormalizer, openLocalShell } from '../src/ssh/local-shell.js';

const require = createRequire(import.meta.url);
const { Server } = require('ssh2') as typeof import('ssh2');

function makeHostKey(): string {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-pj-key-'));
  const file = path.join(dir, 'k');
  execFileSync('ssh-keygen', ['-t', 'ed25519', '-f', file, '-N', '', '-q']);
  const key = readFileSync(file, 'utf8');
  rmSync(dir, { recursive: true, force: true });
  return key;
}

test('connectWithJumps tunnels through bastion forwardOut to inner SSH server', { timeout: 10_000 }, async () => {
  const key = makeHostKey();
  const forwardedTo: string[] = [];

  const inner = new Server({ hostKeys: [key] }, (client) => {
    client.on('authentication', (ctx) => ctx.accept());
    client.on('ready', () => {
      client.on('session', (accept) => {
        const s = accept();
        s.on('exec', (acceptExec, _r, info) => {
          const st = acceptExec();
          st.write(`inner:${info.command}`);
          st.exit(0);
          st.close();
        });
      });
    });
  });

  const bastion = new Server({ hostKeys: [key] }, (client) => {
    client.on('authentication', (ctx) => ctx.accept());
    client.on('tcpip', (accept, reject, info) => {
      forwardedTo.push(`${info.destIP}:${info.destPort}`);
      const socket = net.connect(info.destPort, info.destIP, () => {
        const channel = accept();
        channel.pipe(socket).pipe(channel);
      });
      socket.on('error', () => reject());
    });
  });

  await new Promise<void>((r) => inner.listen(0, '127.0.0.1', r));
  await new Promise<void>((r) => bastion.listen(0, '127.0.0.1', r));
  const innerPort = (inner.address() as net.AddressInfo).port;
  const bastionPort = (bastion.address() as net.AddressInfo).port;

  try {
    const hops: JumpHop[] = [
      { target: { host: '127.0.0.1', port: bastionPort, username: 'jump', authType: 'password', secret: 'x' }, acceptHostKey: () => true },
      { target: { host: '127.0.0.1', port: innerPort, username: 'inner', authType: 'password', secret: 'y' }, acceptHostKey: () => true },
    ];
    const conn = await connectWithJumps(hops);
    try {
      const out = await new Promise<string>((resolve, reject) => {
        conn.client.exec('id', (err, ch) => {
          if (err) return reject(err);
          let text = '';
          ch.on('data', (d: Buffer) => { text += d.toString('utf8'); });
          ch.on('close', () => resolve(text));
        });
      });
      assert.equal(out, 'inner:id');
      assert.deepEqual(forwardedTo, [`127.0.0.1:${innerPort}`]);
    } finally {
      conn.dispose();
    }
  } finally {
    await new Promise<void>((r) => bastion.close(() => r()));
    await new Promise<void>((r) => inner.close(() => r()));
  }
});

test('openLocalShell runs interactive commands, normalizes bare LF to CRLF, and closes cleanly', { timeout: 10_000 }, async () => {
  const norm = createCrLfNormalizer();
  assert.equal(norm(Buffer.from('中文\n第二行\r\n')).toString('utf8'), '中文\r\n第二行\r\n');
  assert.equal(norm(Buffer.from('split\r')).toString('utf8'), 'split\r');
  assert.equal(norm(Buffer.from('\nnext')).toString('utf8'), '\nnext');

  const chunks: Buffer[] = [];
  let closed = false;
  const shell = openLocalShell({
    onData: (buf) => {
      chunks.push(buf);
    },
    onClose: () => {
      closed = true;
    },
  });
  shell.write('echo TIAN_LOCAL_OK\r');
  const start = Date.now();
  while (!Buffer.concat(chunks).toString('utf8').includes('TIAN_LOCAL_OK')) {
    if (Date.now() - start > 6000) {
      throw new Error(`Local shell did not echo marker: ${Buffer.concat(chunks).toString('utf8')}`);
    }
    await new Promise((r) => setTimeout(r, 40));
  }
  const combined = Buffer.concat(chunks).toString('utf8');
  assert.equal(/(^|[^\r])\n/.test(combined), false, `Found bare LF in local shell output: ${JSON.stringify(combined)}`);
  shell.close();
  const cStart = Date.now();
  while (!closed && Date.now() - cStart < 3000) {
    await new Promise((r) => setTimeout(r, 30));
  }
  assert.equal(closed, true);
});
