import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { resolveDataDir } from '../src/core/paths.js';

test('TIANSHELL_HOME wins over the platform default', () => {
  const dir = resolveDataDir({
    env: { TIANSHELL_HOME: 'D:/data/tianshell' },
    platform: 'win32',
    homedir: 'C:/Users/zq',
  });
  assert.equal(dir, path.resolve('D:/data/tianshell'));
});

test('uses the per-user config directory when no override is set', () => {
  assert.equal(
    resolveDataDir({ env: {}, platform: 'win32', homedir: 'C:/Users/zq', appData: 'C:/Users/zq/AppData/Roaming' }),
    path.join('C:/Users/zq/AppData/Roaming', 'tianshell'),
  );
  assert.equal(
    resolveDataDir({ env: {}, platform: 'darwin', homedir: '/Users/zq' }),
    path.join('/Users/zq', 'Library', 'Application Support', 'tianshell'),
  );
  assert.equal(
    resolveDataDir({ env: {}, platform: 'linux', homedir: '/home/zq' }),
    path.join('/home/zq', '.config', 'tianshell'),
  );
});
