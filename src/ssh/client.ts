import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import type { ClientChannel, ConnectConfig } from 'ssh2';

const require = createRequire(import.meta.url);
const { Client: SshClient } = require('ssh2') as typeof import('ssh2');
type SshClientInstance = InstanceType<typeof SshClient>;

const OUTPUT_LIMIT = 200_000;

export interface SshTarget {
  host: string;
  port: number;
  username: string;
  authType: 'password' | 'privateKey';
  secret: string;
  passphrase?: string;
}

export interface JumpHop {
  target: SshTarget;
  acceptHostKey: (fingerprint: string) => boolean;
}

export interface ConnectedSsh {
  client: SshClientInstance;
  dispose: () => void;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  timedOut: boolean;
  truncated: boolean;
}

export interface ShellSession {
  write(data: string): void;
  resize(cols: number, rows: number): void;
  close(): void;
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

function connectSingleHop(hop: JumpHop, sock?: ClientChannel): Promise<SshClientInstance> {
  return new Promise((resolve, reject) => {
    const { config, mismatch } = buildConnectConfig(hop.target, hop.acceptHostKey);
    if (sock) config.sock = sock;
    const client = new SshClient();
    let settled = false;
    const fail = (err: unknown) => {
      if (settled) return;
      settled = true;
      client.end();
      const fp = mismatch();
      reject(fp ? new HostKeyMismatchError(fp) : err instanceof Error ? err : new Error(String(err)));
    };
    client.once('ready', () => {
      if (settled) return;
      settled = true;
      resolve(client);
    });
    client.once('error', fail);
    client.connect(config);
  });
}

function openJumpStream(client: SshClientInstance, nextHost: SshTarget): Promise<ClientChannel> {
  return new Promise((resolve, reject) => {
    client.forwardOut('127.0.0.1', 0, nextHost.host, nextHost.port, (err, stream) => {
      if (err) reject(err);
      else resolve(stream);
    });
  });
}

/**
 * Connect directly or through ordered ProxyJump bastion hops, cleaning upstream clients on failure.
 */
export async function connectWithJumps(hops: JumpHop[]): Promise<ConnectedSsh> {
  if (!hops.length) throw new Error('至少需要一个 SSH 连接目标');
  const clients: SshClientInstance[] = [];
  const dispose = () => {
    for (const c of [...clients].reverse()) {
      try {
        c.end();
      } catch {
        // Ignore teardown errors on already-closed sockets.
      }
    }
  };
  try {
    let current = await connectSingleHop(hops[0]);
    clients.push(current);
    for (let i = 1; i < hops.length; i += 1) {
      const stream = await openJumpStream(current, hops[i].target);
      current = await connectSingleHop(hops[i], stream);
      clients.push(current);
    }
    return { client: current, dispose };
  } catch (err) {
    dispose();
    throw err;
  }
}

export function execOnClient(client: SshClientInstance, command: string, timeoutMs: number): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let stdout = '';
    let stderr = '';
    let truncated = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve({ stdout, stderr, exitCode: null, timedOut: true, truncated });
    }, timeoutMs);

    client.exec(command, (error, channel) => {
      if (error) {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          reject(error);
        }
        return;
      }
      channel.on('data', (chunk: Buffer) => {
        const next = appendCapped(stdout, chunk.toString('utf8'));
        if (next.length < stdout.length + chunk.length) truncated = true;
        stdout = next;
      });
      channel.stderr.on('data', (chunk: Buffer) => {
        const next = appendCapped(stderr, chunk.toString('utf8'));
        if (next.length < stderr.length + chunk.length) truncated = true;
        stderr = next;
      });
      channel.on('close', (code: number | null) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        const out = truncateOutput(stdout);
        const errText = truncateOutput(stderr);
        resolve({
          stdout: out.text,
          stderr: errText.text,
          exitCode: typeof code === 'number' ? code : null,
          timedOut: false,
          truncated: truncated || out.truncated || errText.truncated,
        });
      });
    });
  });
}

export async function execCommand(
  target: SshTarget,
  command: string,
  timeoutMs: number,
  acceptHostKey: (fingerprint: string) => boolean,
  jumps: JumpHop[] = [],
): Promise<ExecResult> {
  const conn = await connectWithJumps([...jumps, { target, acceptHostKey }]);
  try {
    return await execOnClient(conn.client, command, timeoutMs);
  } finally {
    conn.dispose();
  }
}

export async function openShell(options: {
  target: SshTarget;
  cols: number;
  rows: number;
  acceptHostKey: (fingerprint: string) => boolean;
  onData: (data: Buffer) => void;
  onClose: () => void;
  jumps?: JumpHop[];
}): Promise<ShellSession> {
  const cols = clampSize(options.cols, 80);
  const rows = clampSize(options.rows, 24);
  const conn = await connectWithJumps([...(options.jumps ?? []), { target: options.target, acceptHostKey: options.acceptHostKey }]);
  try {
    return await new Promise<ShellSession>((resolve, reject) => {
      conn.client.shell({ term: 'xterm-256color', cols, rows }, (error, channel) => {
        if (error || !channel) {
          reject(error ?? new Error('没有拿到终端通道'));
          return;
        }
        channel.on('data', (chunk: Buffer) => options.onData(chunk));
        channel.stderr.on('data', (chunk: Buffer) => options.onData(chunk));
        channel.on('close', () => {
          conn.dispose();
          options.onClose();
        });
        resolve({
          write: (data: string) => channel.write(data),
          resize: (c: number, r: number) => channel.setWindow(clampSize(r, 24), clampSize(c, 80), 0, 0),
          close: () => {
            channel.close();
            conn.dispose();
          },
        });
      });
    });
  } catch (err) {
    conn.dispose();
    throw err;
  }
}

function appendCapped(current: string, chunk: string): string {
  if (current.length >= OUTPUT_LIMIT) return current;
  return (current + chunk).slice(0, OUTPUT_LIMIT);
}

function clampSize(value: number, fallback: number): number {
  if (!Number.isInteger(value) || value < 1) return fallback;
  return Math.min(value, 512);
}
