import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';
import * as esbuild from 'esbuild';

test('点或双击指向文件的链接时，列目录失败后打开编辑器', async () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'sftp-open-'));
  const outfile = path.join(dir, 'flow.mjs');
  await esbuild.build({
    entryPoints: [path.join(import.meta.dirname, 'sftp-panel.flow.ts')],
    bundle: true,
    format: 'esm',
    platform: 'node',
    outfile,
    logLevel: 'silent',
  });
  const mod = await import(pathToFileURL(outfile).href) as { runSymlinkOpenFlow: () => Promise<void> };
  await mod.runSymlinkOpenFlow();
});
