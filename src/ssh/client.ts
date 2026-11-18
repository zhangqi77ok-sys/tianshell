import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import type { ClientChannel, ConnectConfig } from 'ssh2';

const require = createRequire(import.meta.url);
const { Client: SshClient } = require('ssh2') as typeof import('ssh2');

const OUTPUT_LIMIT = 200_000;

export interface SshTarget {
  host: string;
  port: number;
  username: string;
  authType: 'password' | 'privateKey';
  secret: string;
  passphrase?: string;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  timedOut: boolean;
  truncated: boolean;
}

export class HostKeyMismatchError extends Error {
  readonly fingerprint: string;

  constructor(fingerprint: string) {
    super(
      `主机密钥指纹和上次保存的不一致（${fingerprint}）。如果这台机器换过密钥，请在编辑主机时清除指纹后再连接。`,
    );
    this.name = 'HostKeyMismatchError';
    this.fingerprint = fingerprint;
  }
}

export function fingerprintOf(key: Buffer): string {
  return `SHA256:${createHash('sha256').update(key).digest('base64').replace(/=+$/g, '')}`;
}

export function truncateOutput(text: string, max = OUTPUT_LIMIT): { text: string; truncated: boolean } {
  if (text.length <= max) return { text, truncated: false };
  return { text: `${text.slice(0, max)}\n…（输出已截断）`, truncated: true };
}

export function explainSshError(err: unknown): string {
  if (err instanceof HostKeyMismatchError) return err.message;
  const message = err instanceof Error ? err.message : String(err);
  if (/ENOTFOUND|EAI_AGAIN|getaddrinfo/i.test(message)) return `找不到主机：${message}`;
  if (/ECONNREFUSED/i.test(message)) return `连接被拒绝：${message}`;
  if (/timed out|timeout/i.test(message)) return `连接超时：${message}`;
  if (/authentication|auth fail|all configured authentication methods failed/i.test(message)) {
    return `认证失败，请检查用户名、密码或私钥：${message}`;
  }
  return message ? `SSH 连接失败：${message}` : 'SSH 连接失败';
}

export function buildConnectConfig(
  target: SshTarget,
  acceptHostKey: (fingerprint: string) => boolean,
): { config: ConnectConfig; mismatch: () => string | null } {
  let mismatch: string | null = null;
  const config: ConnectConfig = {
    host: target.host,
    port: target.port,
    username: target.username,
    readyTimeout: 20_000,
    keepaliveInterval: 10_000,
    ident: 'TianShell',
    hostVerifier: ((key: Buffer) => {
      const fingerprint = fingerprintOf(key);
      const accepted = acceptHostKey(fingerprint);
      if (!accepted) mismatch = fingerprint;
      return accepted;
    }) as ConnectConfig['hostVerifier'],
  };
  if (target.authType === 'password') {
    config.password = target.secret;
  } else {
    config.privateKey = target.secret;
    if (target.passphrase) config.passphrase = target.passphrase;
  }
  return { config, mismatch: () => mismatch };
}

export function execCommand(
  target: SshTarget,
  command: string,
  timeoutMs: number,
  acceptHostKey: (fingerprint: string) => boolean,
): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    const { config, mismatch } = buildConnectConfig(target, acceptHostKey);
    const client = new SshClient();
    let settled = false;
    let stdout = '';
    let stderr = '';
    let truncated = false;

    const finish = (result: ExecResult | Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.end();
      if (result instanceof Error) reject(result);
      else resolve(result);
    };

    const timer = setTimeout(() => {
      const out = truncateOutput(stdout);
      const err = truncateOutput(stderr);
      finish({
        stdout: out.text,
        stderr: err.text,
        exitCode: null,
        timedOut: true,
        truncated: truncated || out.truncated || err.truncated,
      });
    }, timeoutMs);

    client.on('ready', () => {
      client.exec(command, (error, stream) => {
        if (error || !stream) {
          finish(error ?? new Error('没有拿到命令通道'));
          return;
        }
        stream.on('data', (chunk: Buffer) => {
          stdout = appendCapped(stdout, chunk.toString('utf8'));
          if (stdout.length >= OUTPUT_LIMIT) truncated = true;
        });
        stream.stderr.on('data', (chunk: Buffer) => {
          stderr = appendCapped(stderr, chunk.toString('utf8'));
          if (stderr.length >= OUTPUT_LIMIT) truncated = true;
        });
        stream.on('close', (code: number | null) => {
          const out = truncateOutput(stdout);
          const errText = truncateOutput(stderr);
          finish({
            stdout: out.text,
            stderr: errText.text,
            exitCode: typeof code === 'number' ? code : null,
            timedOut: false,
            truncated: truncated || out.truncated || errText.truncated,
          });
        });
      });
    });

    client.on('error', (error: Error) => {
      const fingerprint = mismatch();
      finish(fingerprint ? new HostKeyMismatchError(fingerprint) : error);
    });

    client.connect(config);
  });
}

export interface ShellSession {
  write(data: string): void;
  resize(cols: number, rows: number): void;
  close(): void;
}

export function openShell(options: {
  target: SshTarget;
  cols: number;
  rows: number;
  acceptHostKey: (fingerprint: string) => boolean;
  onData: (data: Buffer) => void;
  onClose: () => void;
}): Promise<ShellSession> {
  const cols = clampSize(options.cols, 80);
  const rows = clampSize(options.rows, 24);
  return new Promise((resolve, reject) => {
    const { config, mismatch } = buildConnectConfig(options.target, options.acceptHostKey);
    const client = new SshClient();
    let settled = false;

    const fail = (error: unknown) => {
      if (settled) return;
      settled = true;
      client.end();
      const fingerprint = mismatch();
      reject(fingerprint ? new HostKeyMismatchError(fingerprint) : error instanceof Error ? error : new Error(String(error)));
    };

    client.on('ready', () => {
      client.shell({ term: 'xterm-256color', cols, rows }, (error, channel) => {
        if (error || !channel) {
          fail(error ?? new Error('没有拿到终端通道'));
          return;
        }
        channel.on('data', (chunk: Buffer) => options.onData(chunk));
        channel.stderr.on('data', (chunk: Buffer) => options.onData(chunk));
        channel.on('close', () => {
          client.end();
          options.onClose();
        });
        if (settled) {
          channel.close();
          return;
        }
        settled = true;
        resolve(bindChannel(client, channel));
      });
    });

    client.on('error', (error: Error) => {
      if (!settled) fail(error);
    });
    client.connect(config);
  });
}

function bindChannel(client: InstanceType<typeof SshClient>, channel: ClientChannel): ShellSession {
  return {
    write(data: string) {
      channel.write(data);
    },
    resize(cols: number, rows: number) {
      channel.setWindow(clampSize(rows, 24), clampSize(cols, 80), 0, 0);
    },
    close() {
      channel.close();
      client.end();
    },
  };
}

function appendCapped(current: string, chunk: string): string {
  if (current.length >= OUTPUT_LIMIT) return current;
  return (current + chunk).slice(0, OUTPUT_LIMIT);
}

function clampSize(value: number, fallback: number): number {
  if (!Number.isInteger(value) || value < 1) return fallback;
  return Math.min(value, 512);
}
