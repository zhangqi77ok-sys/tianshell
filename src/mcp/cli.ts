import { chmodSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { createNodeSafeStorage } from '../core/dpapi.js';
import { HostStore } from '../core/host-store.js';
import { loadOrCreateKey } from '../core/key-file.js';
import { resolveDataDir } from '../core/paths.js';
import { SessionRegistry } from '../core/sessions.js';
import { execCommand } from '../ssh/client.js';
import { startStdioServer } from './server.js';

async function main(): Promise<void> {
  // Redirect console.log to stderr so MCP stdout carries JSON-RPC frames exclusively.
  console.log = console.error;

  const dataDir = resolveDataDir();
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  try {
    chmodSync(dataDir, 0o700);
  } catch {
    // Windows relies on DPAPI protection rather than POSIX mode bits.
  }

  const key = loadOrCreateKey(path.join(dataDir, 'vault.key'), createNodeSafeStorage());
  const hosts = new HostStore(path.join(dataDir, 'hosts.json'), key);
  const sessions = new SessionRegistry();

  process.stdin.on('end', () => {
    process.exitCode = 0;
  });

  await startStdioServer({ hosts, sessions, execCommand });
  console.error('TianShell MCP (Node) 已启动');
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
