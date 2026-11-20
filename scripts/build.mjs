import * as esbuild from 'esbuild';
import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export async function build(options = { quiet: false }) {
  const ui = path.join(root, 'ui');
  mkdirSync(ui, { recursive: true });

  await esbuild.build({
    bundle: true,
    sourcemap: false,
    logLevel: options.quiet ? 'warning' : 'info',
    entryPoints: [path.join(root, 'src/renderer/renderer.ts')],
    platform: 'browser',
    format: 'iife',
    outfile: path.join(ui, 'renderer.js'),
    loader: { '.css': 'css' },
  });

  copyFileSync(path.join(root, 'src/renderer/index.html'), path.join(ui, 'index.html'));
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  await build();
}
