import { execFile, spawn } from 'node:child_process';
import { homedir } from 'node:os';
import type { ShellSession } from './client.js';

export interface LocalShellOptions {
  cwd?: string;
  onData: (data: Buffer) => void;
  onClose: () => void;
}

/**
 * Build a stateful stream normalizer that upgrades bare LF (0x0a) to CRLF across chunks
 * so pipe-backed local shells return the cursor to column 0 in xterm.js.
 */
export function createCrLfNormalizer(): (chunk: Buffer) => Buffer {
  let prevWasCr = false;
  return (chunk: Buffer): Buffer => {
    if (chunk.length === 0) return chunk;
    let inserts = 0;
    let scanPrev = prevWasCr;
    for (let i = 0; i < chunk.length; i += 1) {
      const b = chunk[i];
      if (b === 0x0a && !scanPrev) inserts += 1;
      scanPrev = b === 0x0d;
    }
    if (inserts === 0) {
      prevWasCr = scanPrev;
      return chunk;
    }
    const out = Buffer.allocUnsafe(chunk.length + inserts);
    let pos = 0;
    for (let i = 0; i < chunk.length; i += 1) {
      const b = chunk[i];
      if (b === 0x0a && !prevWasCr) {
        out[pos++] = 0x0d;
      }
      out[pos++] = b;
      prevWasCr = b === 0x0d;
    }
    return out;
  };
}

function terminateShellProcess(proc: ReturnType<typeof spawn>, isWin: boolean, onDone: () => void): void {
  try {
    proc.stdin?.end();
  } catch {
    // Ignore pipe shutdown errors.
  }
  if (isWin && proc.pid) {
    execFile('taskkill', ['/F', '/T', '/PID', String(proc.pid)], { windowsHide: true }, onDone);
    return;
  }
  proc.kill('SIGTERM');
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

  // stdout 与 stderr 各自持有独立的换行状态，防止跨管道分片污染 CRLF 判断
  const normOut = createCrLfNormalizer();
  const normErr = createCrLfNormalizer();
  proc.stdout?.on('data', (chunk: Buffer) => options.onData(normOut(chunk)));
  proc.stderr?.on('data', (chunk: Buffer) => options.onData(normErr(chunk)));
  proc.on('close', finish);
  proc.on('error', (err) => {
    options.onData(Buffer.from(`\r\n本地终端错误: ${err.message}\r\n`, 'utf8'));
    finish();
  });

  return {
    write(data: string): void {
      if (!closed && proc.stdin && !proc.stdin.destroyed) {
        // 先合并 \r\n 再转 \n，避免粘贴 CRLF 文本被误拆成两次回车
        proc.stdin.write(isWin ? data.replace(/\r\n?/g, '\n') : data);
      }
    },
    resize(): void {
      // Pipe-backed local shell does not expose TIOCSWINSZ ioctl.
    },
    close(): void {
      if (!closed) terminateShellProcess(proc, isWin, finish);
    },
  };
}
