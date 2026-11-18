import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from './build.mjs';

const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

await build();

const electronPath = require('electron');
const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;

const child = spawn(electronPath, [path.join(root, 'dist', 'main.js'), '--mcp'], {
  stdio: ['inherit', 'pipe', 'inherit'],
  windowsHide: true,
  env,
});

let buffer = '';

function forward(line) {
  const trimmed = line.trim();
  if (!trimmed) return;
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) process.stdout.write(`${line}\n`);
  else process.stderr.write(`${line}\n`);
}

child.stdout.on('data', (chunk) => {
  buffer += chunk.toString('utf8');
  let newline = buffer.indexOf('\n');
  while (newline !== -1) {
    forward(buffer.slice(0, newline));
    buffer = buffer.slice(newline + 1);
    newline = buffer.indexOf('\n');
  }
});

child.on('exit', (code, signal) => {
  if (buffer.trim()) forward(buffer.replace(/\n$/, ''));
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

child.on('error', (error) => {
  console.error(error.message);
  process.exit(1);
});
