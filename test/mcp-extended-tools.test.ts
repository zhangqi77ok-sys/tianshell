import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { resolveHostChain } from '../src/core/host-hops.js';
import { HostStore } from '../src/core/host-store.js';
import { SessionRegistry } from '../src/core/sessions.js';
import { createMcpServer, type ToolDeps } from '../src/mcp/server.js';

test('resolveHostChain orders bastion hops before target', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-hops-'));
  try {
    const hosts = new HostStore(path.join(dir, 'hosts.json'), randomBytes(32));
    const bastion = hosts.save({ alias: 'bastion', host: '1.1.1.1', username: 'jump', authType: 'password', secret: 'a' });
    hosts.save({ alias: 'db', host: '10.0.0.5', username: 'pg', authType: 'password', secret: 'b', jumpHostId: bastion.id });
    const chain = resolveHostChain(hosts, 'db');
    assert.ok(chain);
    assert.equal(chain.hops.length, 2);
    assert.equal(chain.hops[0]?.target.host, '1.1.1.1');
    assert.equal(chain.hops[1]?.target.host, '10.0.0.5');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('MCP exposes sftp_list, sftp_read, sftp_write, and system_metrics tools', async () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-mcp-ext-'));
  try {
    const hosts = new HostStore(path.join(dir, 'hosts.json'), randomBytes(32));
    hosts.save({ alias: 'web', host: '10.0.0.9', username: 'www', authType: 'password', secret: 'pw' });
    const files = new Map<string, string>([['/etc/app.env', 'PORT=3000\n']]);

    const deps: ToolDeps = {
      hosts,
      sessions: new SessionRegistry(),
      execCommand: async () => ({ stdout: 'ok', stderr: '', exitCode: 0, timedOut: false, truncated: false }),
      opsOverride: {
        listDir: async (_h, dirPath) => [
          { name: 'app.env', path: `${dirPath}/app.env`, kind: 'file', size: 10, mtimeMs: 1000, mode: 0o644, permissions: '-rw-r--r--' },
        ],
        readText: async (_h, filePath) => ({ content: files.get(filePath) ?? '', truncated: false }),
        writeText: async (_h, filePath, content) => { files.set(filePath, content); },
        sampleMetrics: async () => ({
          sampledAt: '2026-11-18T00:00:00Z',
          hostname: 'web-01',
          uptimeSeconds: 1200,
          loadAvg: [0.2, 0.3, 0.25],
          cpuPercent: 18.5,
          memTotalKb: 4_000_000,
          memUsedKb: 1_500_000,
          swapTotalKb: 0,
          swapUsedKb: 0,
          disks: [],
          net: { rxBytesPerSec: 100, txBytesPerSec: 200, totalRxBytes: 1000, totalTxBytes: 2000 },
          processes: [],
        }),
      },
    };

    const server = createMcpServer(deps);
    const client = new Client({ name: 'test', version: '1.0' });
    const [ct, st] = InMemoryTransport.createLinkedPair();
    await Promise.all([client.connect(ct), server.connect(st)]);
    try {
      const tools = await client.listTools();
      const names = tools.tools.map((t) => t.name).sort();
      assert.deepEqual(names, ['list_hosts', 'run_command', 'session_status', 'sftp_list', 'sftp_read', 'sftp_write', 'system_metrics']);

      const listRes = await client.callTool({ name: 'sftp_list', arguments: { host: 'web', path: '/etc' } });
      assert.match((listRes.content as Array<{ text: string }>)[0].text, /app\.env/);

      await client.callTool({ name: 'sftp_write', arguments: { host: 'web', path: '/etc/app.env', content: 'PORT=8080\n' } });
      const readRes = await client.callTool({ name: 'sftp_read', arguments: { host: 'web', path: '/etc/app.env' } });
      assert.match((readRes.content as Array<{ text: string }>)[0].text, /PORT=8080/);

      const metRes = await client.callTool({ name: 'system_metrics', arguments: { host: 'web' } });
      assert.match((metRes.content as Array<{ text: string }>)[0].text, /web-01/);
    } finally {
      await client.close();
      await server.close();
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
