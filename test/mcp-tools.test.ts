import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { HostStore } from '../src/core/host-store.js';
import { SessionRegistry } from '../src/core/sessions.js';
import { createMcpServer, type ToolDeps } from '../src/mcp/server.js';
import type { ExecResult, SshTarget } from '../src/ssh/client.js';

interface CallRecord {
  target: SshTarget;
  command: string;
  timeoutMs: number;
}

function fixture(exec?: ToolDeps['execCommand']): {
  deps: ToolDeps;
  calls: CallRecord[];
  dir: string;
  hosts: HostStore;
} {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-mcp-'));
  const hosts = new HostStore(path.join(dir, 'hosts.json'), randomBytes(32));
  hosts.save({
    alias: 'dev',
    host: '10.0.0.8',
    username: 'ubuntu',
    authType: 'password',
    secret: 'hidden-password',
  });
  const calls: CallRecord[] = [];
  const deps: ToolDeps = {
    hosts,
    sessions: new SessionRegistry(),
    execCommand:
      exec ??
      (async (target, command, timeoutMs) => {
        calls.push({ target, command, timeoutMs });
        const result: ExecResult = {
          stdout: 'Linux dev\n',
          stderr: '',
          exitCode: 0,
          timedOut: false,
          truncated: false,
        };
        return result;
      }),
  };
  return { deps, calls, dir, hosts };
}

async function withClient(deps: ToolDeps, run: (client: Client) => Promise<void>): Promise<void> {
  const server = createMcpServer(deps);
  const client = new Client({ name: 'tianshell-test', version: '0.0.0' });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([client.connect(clientTransport), server.connect(serverTransport)]);
  try {
    await run(client);
  } finally {
    await client.close();
    await server.close();
  }
}

function textOf(result: unknown): string {
  assert.ok(result && typeof result === 'object' && 'content' in result && Array.isArray(result.content));
  const block = result.content.find(
    (item) => item && typeof item === 'object' && 'type' in item && item.type === 'text' && 'text' in item,
  );
  assert.ok(block && typeof block === 'object' && 'text' in block && typeof block.text === 'string');
  return block.text;
}

function failed(result: unknown): boolean {
  return Boolean(result && typeof result === 'object' && 'isError' in result && result.isError);
}

test('lists hosts and the three tools without secrets', async () => {
  const { deps, dir } = fixture();
  try {
    await withClient(deps, async (client) => {
      const tools = await client.listTools();
      const names = tools.tools.map((tool) => tool.name).sort();
      assert.deepEqual(names, ['list_hosts', 'run_command', 'session_status', 'sftp_list', 'sftp_read', 'sftp_write', 'system_metrics']);
      const listed = await client.callTool({ name: 'list_hosts', arguments: {} });
      const body = textOf(listed);
      assert.match(body, /dev/);
      assert.equal(body.includes('hidden-password'), false);
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('runs a command on a saved host and records the session', async () => {
  const { deps, calls, dir } = fixture();
  try {
    await withClient(deps, async (client) => {
      const result = await client.callTool({
        name: 'run_command',
        arguments: { host: 'dev', command: 'uname -a', timeout_seconds: 12 },
      });
      const payload = JSON.parse(textOf(result)) as {
        exitCode: number;
        stdout: string;
        sessionId: string;
      };
      assert.equal(payload.exitCode, 0);
      assert.match(payload.stdout, /Linux/);
      assert.equal(calls[0]?.command, 'uname -a');
      assert.equal(calls[0]?.timeoutMs, 12_000);
      assert.equal(calls[0]?.target.secret, 'hidden-password');
      assert.equal(JSON.stringify(payload).includes('hidden-password'), false);

      const status = JSON.parse(
        textOf(await client.callTool({ name: 'session_status', arguments: { session_id: payload.sessionId } })),
      ) as { status: string; exitCode: number; command: string };
      assert.equal(status.status, 'closed');
      assert.equal(status.exitCode, 0);
      assert.equal(status.command, 'uname -a');
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('reports an unknown host and a timeout without throwing away the session', async () => {
  const { deps, dir } = fixture(async () => ({
    stdout: '',
    stderr: '',
    exitCode: null,
    timedOut: true,
    truncated: false,
  }));
  try {
    await withClient(deps, async (client) => {
      const missing = await client.callTool({
        name: 'run_command',
        arguments: { host: 'nope', command: 'true' },
      });
      assert.equal(failed(missing), true);
      assert.match(textOf(missing), /没有找到主机/);

      const timed = await client.callTool({
        name: 'run_command',
        arguments: { host: 'dev', command: 'sleep 99' },
      });
      const payload = JSON.parse(textOf(timed)) as { timedOut: boolean; sessionId: string };
      assert.equal(payload.timedOut, true);
      const all = JSON.parse(textOf(await client.callTool({ name: 'session_status', arguments: {} }))) as Array<{
        id: string;
      }>;
      assert.equal(all.some((item) => item.id === payload.sessionId), true);
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('rejects an empty command', async () => {
  const { deps, dir } = fixture();
  try {
    await withClient(deps, async (client) => {
      const result = await client.callTool({
        name: 'run_command',
        arguments: { host: 'dev', command: '   ' },
      });
      assert.equal(failed(result), true);
      assert.match(textOf(result), /命令/);
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
