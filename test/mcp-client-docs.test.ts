import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const root = process.cwd();

test('mcp docs have copy-paste Cursor and Claude Code configs', () => {
  const doc = readFileSync(path.join(root, 'docs/mcp.md'), 'utf8');
  assert.match(doc, /"url": "http:\/\/127\.0\.0\.1:53921\/mcp"/);
  assert.match(doc, /"type": "http"/);
  assert.match(doc, /\$\{env:TIANSHELL_TOKEN\}/);
  assert.match(doc, /\$\{TIANSHELL_TOKEN\}/);
  assert.match(doc, /list_terminal_sessions/);
  assert.match(doc, /claude mcp add --transport http/);
  assert.doesNotMatch(doc, /BEGIN OPENSSH PRIVATE KEY/);
  assert.doesNotMatch(doc, /[0-9a-f]{32,}/i);
});

test('workbench page surfaces the MCP connection dialog', () => {
  const html = readFileSync(path.join(root, 'src/renderer/index.html'), 'utf8');
  const source = readFileSync(path.join(root, 'src/renderer/renderer.ts'), 'utf8');
  assert.match(html, /id="open-mcp"/);
  assert.match(html, /id="mcp-dialog"/);
  assert.match(html, /id="mcp-status"/);
  assert.match(html, /Cursor/);
  assert.match(html, /Claude Code/);
  assert.match(source, /type: 'http'/);
  assert.doesNotMatch(source, /console\.log\([^)]*token/i);
});
