import { spawn } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from './build.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const mcpEntry = path.join(root, 'dist', 'mcp.js');

function latestMtimeMs(dir) {
  let latest = 0;
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, item.name);
    if (item.isDirectory()) {
      latest = Math.max(latest, latestMtimeMs(full));
    } else if (item.isFile()) {
      latest = Math.max(latest, statSync(full).mtimeMs);
    }
  }
  return latest;
}

if (!existsSync(mcpEntry) || latestMtimeMs(path.join(root, 'src')) > statSync(mcpEntry).mtimeMs) {
  await build({ quiet: true });
}

const child = spawn(process.execPath, [mcpEntry], {
  cwd: root,
  env: process.env,
  stdio: 'inherit',
  windowsHide: true,
});

const forwardSignal = (sig) => {
  if (child.exitCode === null) child.kill(sig);
};
process.on('SIGINT', () => forwardSignal('SIGINT'));
process.on('SIGTERM', () => forwardSignal('SIGTERM'));

child.on('close', (code) => {
  process.exitCode = code ?? 1;
});

child.on('error', (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
