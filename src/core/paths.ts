import os from 'node:os';
import path from 'node:path';

export function resolveDataDir(options?: {
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
  homedir?: string;
  appData?: string;
}): string {
  const env = options?.env ?? process.env;
  const platform = options?.platform ?? process.platform;
  const homedir = options?.homedir ?? os.homedir();
  if (env.TIANSHELL_HOME) {
    return path.resolve(env.TIANSHELL_HOME);
  }
  if (platform === 'win32') {
    const base = options?.appData ?? env.APPDATA ?? path.join(homedir, 'AppData', 'Roaming');
    return path.join(base, 'tianshell');
  }
  if (platform === 'darwin') {
    return path.join(homedir, 'Library', 'Application Support', 'tianshell');
  }
  const configHome = env.XDG_CONFIG_HOME || path.join(homedir, '.config');
  return path.join(configHome, 'tianshell');
}
