import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

function requestLine(proc: ReturnType<typeof spawn>, payload: Record<string, unknown>, timeoutMs = 15_000): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`Timeout waiting for MCP id=${payload.id}`)), timeoutMs);
    let buf = '';
    const onData = (chunk: Buffer) => {
      buf += chunk.toString('utf8');
      const idx = buf.indexOf('\n');
      if (idx === -1) return;
      const line = buf.slice(0, idx).trim();
      if (!line) return;
      try {
        const parsed = JSON.parse(line) as Record<string, unknown>;
        if (parsed.id === payload.id) {
          clearTimeout(timer);
          proc.stdout?.off('data', onData);
          resolve(parsed);
        }
      } catch {
        // Ignore non-JSON lines if any
      }
    };
    proc.stdout?.on('data', onData);
    proc.stdin?.write(`${JSON.stringify(payload)}\n`);
  });
}

test('pure Node scripts/mcp.mjs handles live stdio MCP handshake without Electron', async () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-pure-mcp-'));
  const proc = spawn(process.execPath, ['scripts/mcp.mjs'], {
    cwd: process.cwd(),
    env: { ...process.env, TIANSHELL_HOME: dir },
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let stderr = '';
  proc.stderr?.on('data', (chunk: Buffer) => {
    stderr += chunk.toString('utf8');
  });

  try {
    const init = await requestLine(proc, {
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'test', version: '1.0' } },
    });
    assert.ok('result' in init, `initialize failed: ${stderr}`);
    proc.stdin?.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);

    const toolsResp = await requestLine(proc, { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} });
    const toolsResult = toolsResp.result as { tools: Array<{ name: string }> };
    const names = toolsResult.tools.map((t) => t.name).sort();
    assert.deepEqual(names, ['list_hosts', 'run_command', 'session_status', 'sftp_list', 'sftp_read', 'sftp_write', 'system_metrics']);

    const hostsResp = await requestLine(proc, {
      jsonrpc: '2.0',
      id: 3,
      method: 'tools/call',
      params: { name: 'list_hosts', arguments: {} },
    });
    const content = (hostsResp.result as { content: Array<{ text: string }> }).content;
    assert.equal(content[0]?.text, '[]');

    // Pure Node DPAPI key must have been created on Windows (never Electron TS1S)
    if (process.platform === 'win32') {
      const magic = readFileSync(path.join(dir, 'vault.key')).subarray(0, 4).toString('utf8');
      assert.equal(magic, 'TS1D');
    }
  } finally {
    proc.stdin?.end();
    await new Promise<void>((res) => {
      proc.once('close', () => res());
      setTimeout(() => {
        proc.kill();
        res();
      }, 2000);
    });
    rmSync(dir, { recursive: true, force: true });
  }
});
