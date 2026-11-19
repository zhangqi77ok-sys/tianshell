import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import net from 'node:net';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseMetricsSample } from '../src/ssh/monitor.js';
import { TunnelManager } from '../src/ssh/tunnel.js';

const require = createRequire(import.meta.url);
const { Server } = require('ssh2') as typeof import('ssh2');

function makeKey(): string {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-tun-k-'));
  const f = path.join(dir, 'k');
  execFileSync('ssh-keygen', ['-t', 'ed25519', '-f', f, '-N', '', '-q']);
  const k = readFileSync(f, 'utf8');
  rmSync(dir, { recursive: true, force: true });
  return k;
}

test('parseMetricsSample computes accurate CPU, memory, swap, disk, network delta, and top processes', () => {
  const sample1 = [
    '__HOST__',
    'prod-node-1',
    '__UPTIME__',
    '86400.50 12345.00',
    '__LOAD__',
    '0.45 0.60 0.55 2/310 1982',
    '__CPU__',
    'cpu  1000 0 200 8800 0 0 0 0 0 0',
    '__MEM__',
    'MemTotal:        8000000 kB',
    'MemFree:         1000000 kB',
    'MemAvailable:    5000000 kB',
    'SwapTotal:       2000000 kB',
    'SwapFree:        1500000 kB',
    '__NET__',
    'lo: 500 1 0 0 0 0 0 0 500 1 0 0 0 0 0 0',
    'eth0: 10000 100 0 0 0 0 0 0 20000 200 0 0 0 0 0 0',
    '__DF__',
    'Filesystem     1024-blocks    Used Available Capacity Mounted on',
    '/dev/vda1         40000000 10000000  30000000      25% /',
    '__PS__',
    '  PID USER     %CPU %MEM COMMAND',
    '  1024 www      35.2  4.1 node',
  ].join('\n');

  const sample2 = sample1
    .replace('cpu  1000 0 200 8800', 'cpu  1500 0 300 9200') // total +1000, idle +400 -> 60% CPU
    .replace('eth0: 10000 100 0 0 0 0 0 0 20000', 'eth0: 14000 110 0 0 0 0 0 0 28000');

  const first = parseMetricsSample(sample1, undefined, 1000);
  const second = parseMetricsSample(sample2, first.counters, 3000); // 2 seconds delta -> rx=2000 B/s, tx=4000 B/s

  assert.equal(second.metrics.hostname, 'prod-node-1');
  assert.equal(second.metrics.uptimeSeconds, 86400);
  assert.deepEqual(second.metrics.loadAvg, [0.45, 0.6, 0.55]);
  assert.equal(Math.round(second.metrics.cpuPercent), 60);
  assert.equal(second.metrics.memUsedKb, 3_000_000);
  assert.equal(second.metrics.swapUsedKb, 500_000);
  assert.equal(second.metrics.disks[0]?.mount, '/');
  assert.equal(second.metrics.net.rxBytesPerSec, 2000);
  assert.equal(second.metrics.net.txBytesPerSec, 4000);
  assert.equal(second.metrics.processes[0]?.command, 'node');
});

test('TunnelManager supports live local (-L) and dynamic SOCKS5 (-D) forwarding over SSH', { timeout: 10_000 }, async () => {
  // Echo TCP backend behind SSH server
  const backend = net.createServer((sock) => {
    sock.on('data', (chunk) => sock.write(Buffer.concat([Buffer.from('ECHO:'), chunk])));
  });
  await new Promise<void>((r) => backend.listen(0, '127.0.0.1', r));
  const backendPort = (backend.address() as net.AddressInfo).port;

  const sshServer = new Server({ hostKeys: [makeKey()] }, (client) => {
    client.on('authentication', (ctx) => ctx.accept());
    client.on('tcpip', (accept, reject, info) => {
      const targetSock = net.connect(info.destPort, info.destIP, () => {
        const ch = accept();
        ch.pipe(targetSock).pipe(ch);
      });
      targetSock.on('error', () => reject());
    });
  });
  await new Promise<void>((r) => sshServer.listen(0, '127.0.0.1', r));
  const sshPort = (sshServer.address() as net.AddressInfo).port;

  const manager = new TunnelManager();
  try {
    const hops = [
      { target: { host: '127.0.0.1', port: sshPort, username: 'u', authType: 'password' as const, secret: 'p' }, acceptHostKey: () => true },
    ];

    // 1. Local port forward (-L)
    const localRule = {
      id: 't-local',
      name: 'Local Echo',
      hostId: 'h1',
      mode: 'local' as const,
      localHost: '127.0.0.1',
      localPort: 0,
      remoteHost: '127.0.0.1',
      remotePort: backendPort,
    };
    const boundLocalPort = await manager.start(localRule, hops);
    const localReply = await new Promise<string>((resolve, reject) => {
      const c = net.connect(boundLocalPort, '127.0.0.1', () => c.write('ping-L'));
      c.once('data', (d) => { c.destroy(); resolve(d.toString('utf8')); });
      c.once('error', reject);
    });
    assert.equal(localReply, 'ECHO:ping-L');

    // 2. SOCKS5 dynamic proxy (-D)
    const socksRule = {
      id: 't-socks',
      name: 'SOCKS5',
      hostId: 'h1',
      mode: 'socks5' as const,
      localHost: '127.0.0.1',
      localPort: 0,
    };
    const boundSocksPort = await manager.start(socksRule, hops);
    const socksReply = await new Promise<string>((resolve, reject) => {
      const c = net.connect(boundSocksPort, '127.0.0.1', () => {
        // Greeting: VER=5, NMETHODS=1, METHOD=0x00 (No Auth)
        c.write(Buffer.from([0x05, 0x01, 0x00]));
      });
      let stage = 0;
      c.on('data', (chunk) => {
        if (stage === 0) {
          assert.deepEqual(chunk.subarray(0, 2), Buffer.from([0x05, 0x00]));
          stage = 1;
          const req = Buffer.from([0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, (backendPort >> 8) & 0xff, backendPort & 0xff]);
          c.write(req);
        } else if (stage === 1) {
          assert.equal(chunk[1], 0x00);
          stage = 2;
          c.write('ping-D');
        } else {
          c.destroy();
          resolve(chunk.toString('utf8'));
        }
      });
      c.once('error', reject);
    });
    assert.equal(socksReply, 'ECHO:ping-D');
  } finally {
    await manager.stopAll();
    await new Promise<void>((r) => sshServer.close(() => r()));
    await new Promise<void>((r) => backend.close(() => r()));
  }
});
