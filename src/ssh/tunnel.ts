import net from 'node:net';
import type { ClientChannel } from 'ssh2';
import type { TunnelRule, TunnelRuntime } from '../shared/types.js';
import { connectWithJumps, type ConnectedSsh, type JumpHop } from './client.js';

interface ActiveTunnel {
  rule: TunnelRule;
  ssh: ConnectedSsh;
  server?: net.Server;
  boundPort: number;
  connections: number;
  close: () => Promise<void>;
}

function wirePair(sock: net.Socket, channel: ClientChannel, onDone: () => void): void {
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    sock.destroy();
    try {
      channel.close();
    } catch {
      // Channel already closed.
    }
    onDone();
  };
  sock.on('error', finish);
  channel.on('error', finish);
  sock.once('close', finish);
  channel.once('close', finish);
  sock.pipe(channel).pipe(sock);
}

function parseSocks5Target(buf: Buffer): { host: string; port: number } | null {
  if (buf.length < 7 || buf[0] !== 0x05 || buf[1] !== 0x01) return null;
  const atyp = buf[3];
  if (atyp === 0x01 && buf.length >= 10) {
    return { host: `${buf[4]}.${buf[5]}.${buf[6]}.${buf[7]}`, port: buf.readUInt16BE(8) };
  }
  if (atyp === 0x03) {
    const len = buf[4];
    if (buf.length < 7 + len) return null;
    return { host: buf.subarray(5, 5 + len).toString('utf8'), port: buf.readUInt16BE(5 + len) };
  }
  if (atyp === 0x04 && buf.length >= 22) {
    const parts: string[] = [];
    for (let i = 0; i < 16; i += 2) parts.push(buf.readUInt16BE(4 + i).toString(16));
    return { host: parts.join(':'), port: buf.readUInt16BE(20) };
  }
  return null;
}

export class TunnelManager {
  private readonly active = new Map<string, ActiveTunnel>();
  private readonly errors = new Map<string, string>();

  async start(rule: TunnelRule, hops: JumpHop[]): Promise<number> {
    await this.stop(rule.id);
    this.errors.delete(rule.id);
    const ssh = await connectWithJumps(hops);
    try {
      if (rule.mode === 'remote') {
        return await this.startRemote(rule, ssh);
      }
      return await this.startLocalOrSocks(rule, ssh);
    } catch (err) {
      ssh.dispose();
      const msg = err instanceof Error ? err.message : String(err);
      this.errors.set(rule.id, msg);
      throw err;
    }
  }

  private async startLocalOrSocks(rule: TunnelRule, ssh: ConnectedSsh): Promise<number> {
    const entry: ActiveTunnel = {
      rule,
      ssh,
      boundPort: rule.localPort,
      connections: 0,
      close: async () => {
        await new Promise<void>((res) => (entry.server ? entry.server.close(() => res()) : res()));
        ssh.dispose();
      },
    };
    const server = net.createServer((sock) => {
      if (rule.mode === 'socks5') {
        this.handleSocks5(sock, ssh, entry);
      } else {
        this.forwardSocket(sock, ssh, rule.remoteHost ?? '127.0.0.1', rule.remotePort ?? 80, entry);
      }
    });
    entry.server = server;
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(rule.localPort, rule.localHost, () => {
        server.off('error', reject);
        resolve();
      });
    });
    const addr = server.address();
    entry.boundPort = addr && typeof addr !== 'string' ? addr.port : rule.localPort;
    this.active.set(rule.id, entry);
    return entry.boundPort;
  }

  private forwardSocket(sock: net.Socket, ssh: ConnectedSsh, dstHost: string, dstPort: number, entry: ActiveTunnel): void {
    entry.connections += 1;
    ssh.client.forwardOut(sock.remoteAddress || '127.0.0.1', sock.remotePort || 0, dstHost, dstPort, (err, ch) => {
      if (err || !ch) {
        entry.connections = Math.max(0, entry.connections - 1);
        sock.destroy();
        return;
      }
      wirePair(sock, ch, () => {
        entry.connections = Math.max(0, entry.connections - 1);
      });
    });
  }

  private handleSocks5(sock: net.Socket, ssh: ConnectedSsh, entry: ActiveTunnel): void {
    let phase: 'greeting' | 'request' = 'greeting';
    const onError = () => sock.destroy();
    sock.on('error', onError);
    sock.on('data', (chunk: Buffer) => {
      if (phase === 'greeting') {
        if (chunk[0] !== 0x05) return sock.destroy();
        phase = 'request';
        sock.write(Buffer.from([0x05, 0x00]));
        return;
      }
      if (phase === 'request') {
        const target = parseSocks5Target(chunk);
        sock.removeAllListeners('data');
        sock.removeListener('error', onError);
        if (!target) {
          sock.end(Buffer.from([0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]));
          return;
        }
        entry.connections += 1;
        ssh.client.forwardOut('127.0.0.1', 0, target.host, target.port, (err, ch) => {
          if (err || !ch) {
            entry.connections = Math.max(0, entry.connections - 1);
            sock.end(Buffer.from([0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]));
            return;
          }
          sock.write(Buffer.from([0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]));
          wirePair(sock, ch, () => {
            entry.connections = Math.max(0, entry.connections - 1);
          });
        });
      }
    });
  }

  private async startRemote(rule: TunnelRule, ssh: ConnectedSsh): Promise<number> {
    const bindHost = rule.localHost || '127.0.0.1';
    const dstHost = rule.remoteHost || '127.0.0.1';
    const dstPort = rule.remotePort ?? 80;
    const bound = await new Promise<number>((resolve, reject) => {
      ssh.client.forwardIn(bindHost, rule.localPort, (err, port) => {
        if (err) reject(err);
        else resolve(port ?? rule.localPort);
      });
    });
    const entry: ActiveTunnel = {
      rule,
      ssh,
      boundPort: bound,
      connections: 0,
      close: async () => {
        await new Promise<void>((res) => ssh.client.unforwardIn(bindHost, bound, () => res()));
        ssh.dispose();
      },
    };
    ssh.client.on('tcp connection', (_info, accept) => {
      const ch = accept();
      entry.connections += 1;
      const sock = net.connect(dstPort, dstHost, () => {
        wirePair(sock, ch, () => {
          entry.connections = Math.max(0, entry.connections - 1);
        });
      });
      sock.on('error', () => {
        entry.connections = Math.max(0, entry.connections - 1);
        ch.close();
      });
    });
    this.active.set(rule.id, entry);
    return bound;
  }

  async stop(ruleId: string): Promise<void> {
    const existing = this.active.get(ruleId);
    if (!existing) return;
    this.active.delete(ruleId);
    await existing.close();
  }

  async stopAll(): Promise<void> {
    for (const id of [...this.active.keys()]) {
      await this.stop(id);
    }
  }

  getRuntimes(rules: TunnelRule[]): TunnelRuntime[] {
    return rules.map((r) => {
      const act = this.active.get(r.id);
      return {
        ...r,
        localPort: act ? act.boundPort : r.localPort,
        active: Boolean(act),
        connections: act?.connections ?? 0,
        error: this.errors.get(r.id),
      };
    });
  }
}
