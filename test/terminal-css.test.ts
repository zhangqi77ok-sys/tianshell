import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

test('renderer stylesheet includes xterm layout and clips the terminal pane', () => {
  const built = spawnSync(process.execPath, ['scripts/build.mjs'], { cwd: process.cwd(), encoding: 'utf8' });
  assert.equal(built.status, 0, built.stderr);
  const css = readFileSync(path.join(process.cwd(), 'ui', 'renderer.css'), 'utf8');
  assert.match(css, /\.xterm-viewport/);
  assert.match(css, /\.xterm-screen/);
  assert.match(css, /\.xterm-char-measure-element/);
  assert.match(css, /\.terminal-pane\s*\{[^}]*overflow:\s*hidden/);
  assert.doesNotMatch(readFileSync(path.join(process.cwd(), 'src', 'renderer', 'renderer.ts'), 'utf8'), /Cascadia Code/);
});
