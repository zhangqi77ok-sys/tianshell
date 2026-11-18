import { app, BrowserWindow, safeStorage } from 'electron';
import { chmodSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadOrCreateKey, type SafeStorage } from '../core/key-file.js';
import { HostStore } from '../core/host-store.js';
import { resolveDataDir } from '../core/paths.js';
import { SessionRegistry } from '../core/sessions.js';
import { startStdioServer } from '../mcp/server.js';
import { execCommand, type ShellSession } from '../ssh/client.js';
import { registerIpc } from './ipc.js';
import { installMenu } from './menu.js';

const mcpMode = process.argv.includes('--mcp');
const dataDir = resolveDataDir();
const electronDataDir = path.join(dataDir, 'app');

mkdirSync(electronDataDir, { recursive: true, mode: 0o700 });
app.setName('tianshell');
app.setPath('userData', electronDataDir);
app.setAppUserModelId('com.zhangqi77ok.tianshell');

if (mcpMode) {
  app.disableHardwareAcceleration();
  app.commandLine.appendSwitch('disable-gpu');
}

let mainWindow: BrowserWindow | null = null;
const shells = new Map<string, ShellSession>();
const gotLock = mcpMode || app.requestSingleInstanceLock();

if (!gotLock) {
  app.quit();
} else if (!mcpMode) {
  app.on('second-instance', () => {
    if (!mainWindow) return;
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.focus();
  });
}

app.on('window-all-closed', () => {
  if (!mcpMode) app.quit();
});

app.on('activate', () => {
  if (!mcpMode && BrowserWindow.getAllWindows().length === 0) createWindow();
});

app.on('before-quit', () => {
  for (const shell of shells.values()) shell.close();
});

if (gotLock) {
  app
    .whenReady()
    .then(async () => {
      mkdirSync(dataDir, { recursive: true, mode: 0o700 });
      try {
        chmodSync(dataDir, 0o700);
      } catch {
        // Windows 上目录权限不完全靠 chmod。
      }
      const key = loadOrCreateKey(path.join(dataDir, 'vault.key'), electronSafeStorage());
      const hosts = new HostStore(path.join(dataDir, 'hosts.json'), key);
      const sessions = new SessionRegistry();

      if (mcpMode) {
        console.error('TianShell MCP 已启动');
        process.stdin.on('end', () => app.exit(0));
        process.stdin.on('close', () => app.exit(0));
        await startStdioServer({ hosts, sessions, execCommand });
        return;
      }

      installMenu(() => mainWindow);
      registerIpc({
        dataDir,
        hosts,
        sessions,
        shells,
        getWindow: () => mainWindow,
      });
      createWindow();
    })
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error);
      app.exit(1);
    });
}

function createWindow(): void {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const win = new BrowserWindow({
    width: 1120,
    height: 740,
    minWidth: 880,
    minHeight: 560,
    backgroundColor: '#14181d',
    title: 'TianShell',
    show: false,
    webPreferences: {
      preload: path.join(here, 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  mainWindow = win;
  win.once('ready-to-show', () => win.show());
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  win.webContents.on('will-navigate', (event) => event.preventDefault());
  win.on('closed', () => {
    if (mainWindow === win) mainWindow = null;
  });
  void win.loadFile(path.join(here, 'index.html'));
}

function electronSafeStorage(): SafeStorage {
  return {
    isEncryptionAvailable: () => safeStorage.isEncryptionAvailable(),
    encryptString: (plain) => safeStorage.encryptString(plain),
    decryptString: (encrypted) => safeStorage.decryptString(encrypted),
  };
}
