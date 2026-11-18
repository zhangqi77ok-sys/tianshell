import * as esbuild from 'esbuild';
import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export async function build() {
  const dist = path.join(root, 'dist');
  mkdirSync(dist, { recursive: true });
  const shared = {
    bundle: true,
    sourcemap: true,
    logLevel: 'info',
  };
  await esbuild.build({
    ...shared,
    entryPoints: [path.join(root, 'src/main/main.ts')],
    platform: 'node',
    format: 'esm',
    outfile: path.join(dist, 'main.js'),
    external: ['electron', 'ssh2'],
  });
  await esbuild.build({
    ...shared,
    entryPoints: [path.join(root, 'src/main/preload.ts')],
    platform: 'node',
    format: 'cjs',
    outfile: path.join(dist, 'preload.cjs'),
    external: ['electron'],
  });
  await esbuild.build({
    ...shared,
    entryPoints: [path.join(root, 'src/renderer/renderer.ts')],
    platform: 'browser',
    format: 'iife',
    outfile: path.join(dist, 'renderer.js'),
  });
  copyFileSync(path.join(root, 'src/renderer/index.html'), path.join(dist, 'index.html'));
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  await build();
}
