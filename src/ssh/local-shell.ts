import { execFile, spawn } from 'node:child_process';
import { homedir } from 'node:os';
import type { ShellSession } from './client.js';

export interface LocalShellOptions {
  cwd?: string;
  onData: (data: Buffer) => void;
  onClose: () => void;
}

/**
 * Spawn an interactive local OS shell (PowerShell on Windows, $SHELL/sh on POSIX) without native C++ pty dependencies.
 */
export function openLocalShell(options: LocalShellOptions): ShellSession {
  const isWin = process.platform === 'win32';
  const shellCmd = isWin ? 'powershell.exe' : process.env.SHELL || '/bin/sh';
  const args = isWin
    ? ['-NoLogo', '-NoExit', '-Command', '[Console]::InputEncoding=[System.Text.Encoding]::UTF8; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8']
    : ['-i'];

  const proc = spawn(shellCmd, args, {
    cwd: options.cwd || homedir(),
    env: process.env,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });

  let closed = false;
  const finish = () => {
    if (closed) return;
    closed = true;
    options.onClose();
  };

  proc.stdout?.on('data', (chunk: Buffer) => options.onData(chunk));
  proc.stderr?.on('data', (chunk: Buffer) => options.onData(chunk));
  proc.on('close', finish);
  proc.on('error', (err) => {
    options.onData(Buffer.from(`\r\n本地终端错误: ${err.message}\r\n`, 'utf8'));
    finish();
  });

  return {
    write(data: string): void {
      if (!closed && proc.stdin && !proc.stdin.destroyed) {
        proc.stdin.write(isWin ? data.replace(/\r/g, '\n') : data);
      }
    },
    resize(): void {
      // Pipe-backed local shell does not expose TIOCSWINSZ ioctl.
    },
    close(): void {
      if (closed) return;
      try {
        proc.stdin?.end();
      } catch {
        // Ignore pipe shutdown errors.
      }
      if (isWin && proc.pid) {
        execFile('taskkill', ['/F', '/T', '/PID', String(proc.pid)], { windowsHide: true }, () => finish());
      } else {
        proc.kill('SIGTERM');
      }
    },
  };
}
