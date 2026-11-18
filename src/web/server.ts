import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync } from 'node:fs';
import { createServer } from 'node:http';
import type net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createNodeSafeStorage } from '../core/dpapi.js';
import { HostStore } from '../core/host-store.js';
import { loadOrCreateKey } from '../core/key-file.js';
import { resolveDataDir } from '../core/paths.js';
import { SettingsStore, SnippetStore, TunnelStore } from '../core/stores.js';
import { TunnelManager } from '../ssh/tunnel.js';
import { handleHttpRequest, type AppContext } from './api.js';
import { SessionHub } from './ws-hub.js';

export interface ServerOptions {
  dataDir?: string;
  distDir?: string;
  port?: number;
  openBrowser?: boolean;
}

export interface RunningTianShellServer {
  port: number;
  token: string;
  url: string;
  close: () => Promise<void>;
}

const DEFAULT_SNIPPETS = [
  { name: '系统负载与运行时间', category: '诊断', command: 'uptime && free -h' },
  { name: '监听端口检查', category: '网络', command: 'ss -tulnp' },
  { name: '磁盘空间占用', category: '磁盘', command: 'df -hT' },
  { name: '内存占用 Top10', category: '进程', command: 'ps aux --sort=-%mem | head -n 11' },
];

function seedSnippetsIfNew(snippetsPath: string, store: SnippetStore): void {
  if (existsSync(snippetsPath)) return;
  for (const s of DEFAULT_SNIPPETS) store.save(s);
}

function launchBrowserWindow(url: string): void {
  try {
    if (process.platform === 'win32') {
      spawn('cmd', ['/c', 'start', '', url], { detached: true, stdio: 'ignore', windowsHide: true }).unref();
    } else if (process.platform === 'darwin') {
      spawn('open', [url], { detached: true, stdio: 'ignore' }).unref();
    } else {
      spawn('xdg-open', [url], { detached: true, stdio: 'ignore' }).unref();
    }
  } catch {
    // Non-fatal: CLI always prints the clickable URL.
  }
}

export async function createTianShellServer(options: ServerOptions = {}): Promise<RunningTianShellServer> {
  const dataDir = options.dataDir ?? resolveDataDir();
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  try {
    chmodSync(dataDir, 0o700);
  } catch {
    // Windows relies on DPAPI protection.
  }

  const key = loadOrCreateKey(path.join(dataDir, 'vault.key'), createNodeSafeStorage());
  const snippetsPath = path.join(dataDir, 'snippets.json');
  const snippets = new SnippetStore(snippetsPath);
  seedSnippetsIfNew(snippetsPath, snippets);

  const settings = new SettingsStore(path.join(dataDir, 'settings.json'));
  const hub = new SessionHub();
  hub.setMonitorInterval(settings.get().monitorIntervalMs);

  const distDir = options.distDir ?? path.resolve(path.dirname(fileURLToPath(import.meta.url)));
  const ctx: AppContext = {
    hosts: new HostStore(path.join(dataDir, 'hosts.json'), key),
    snippets,
    tunnels: new TunnelStore(path.join(dataDir, 'tunnels.json')),
    settings,
    tunnelMgr: new TunnelManager(),
    hub,
    token: randomBytes(24).toString('hex'),
    distDir,
    dataDir,
    transfers: new Map(),
  };

  const server = createServer((req, res) => {
    void handleHttpRequest(req, res, ctx);
  });
  server.on('upgrade', (req, sock) => {
    hub.handleUpgrade(req, sock as net.Socket, ctx.token);
  });

  const listenPort = options.port ?? 0;
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(listenPort, '127.0.0.1', () => resolve());
  });

  const addr = server.address();
  const actualPort = addr && typeof addr !== 'string' ? addr.port : listenPort;
  const url = `http://127.0.0.1:${actualPort}/?token=${ctx.token}`;

  if (options.openBrowser) launchBrowserWindow(url);

  return {
    port: actualPort,
    token: ctx.token,
    url,
    close: async () => {
      hub.closeAll();
      await ctx.tunnelMgr.stopAll();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

const isCli = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isCli) {
  const portEnv = Number(process.env.TIANSHELL_PORT || 0);
  const noOpen = process.argv.includes('--no-open');
  const app = await createTianShellServer({ port: Number.isInteger(portEnv) ? portEnv : 0, openBrowser: !noOpen });
  console.log(`TianShell Web 工作台已启动 (零 Electron): ${app.url}`);
  const shutdown = async () => {
    await app.close();
    process.exit(0);
  };
  process.on('SIGINT', () => void shutdown());
  process.on('SIGTERM', () => void shutdown());
}
