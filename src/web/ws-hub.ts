import { createHash, randomUUID } from 'node:crypto';
import type { IncomingMessage } from 'node:http';
import type net from 'node:net';
import type { ClientChannel } from 'ssh2';
import type { HostStore } from '../core/host-store.js';
import { resolveHostChain } from '../core/host-hops.js';
import type { ConnectInfo, TransferTask } from '../shared/types.js';
import { connectWithJumps, type ConnectedSsh, type ShellSession } from '../ssh/client.js';
import { openLocalShell } from '../ssh/local-shell.js';
import { sampleServerMetrics, type RawCounters } from '../ssh/monitor.js';

interface ManagedTab {
  id: string;
  kind: 'ssh' | 'local';
  alias: string;
  shell: ShellSession;
  ssh?: ConnectedSsh;
  monitorTimer?: ReturnType<typeof setInterval>;
  prevCounters?: RawCounters;
}

const WS_MAGIC = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

function encodeWsFrame(opcode: number, payload: Buffer): Buffer {
  const len = payload.length;
  if (len < 126) {
    return Buffer.concat([Buffer.from([0x80 | opcode, len]), payload]);
  }
  if (len < 65536) {
    const hdr = Buffer.alloc(4);
    hdr[0] = 0x80 | opcode;
    hdr[1] = 126;
    hdr.writeUInt16BE(len, 2);
    return Buffer.concat([hdr, payload]);
  }
  const hdr = Buffer.alloc(10);
  hdr[0] = 0x80 | opcode;
  hdr[1] = 127;
  hdr.writeBigUInt64BE(BigInt(len), 2);
  return Buffer.concat([hdr, payload]);
}

function parseClientFrames(buf: Buffer): { frames: Array<{ opcode: number; data: Buffer }>; rest: Buffer } {
  const frames: Array<{ opcode: number; data: Buffer }> = [];
  let offset = 0;
  while (buf.length - offset >= 2) {
    const opcode = buf[offset] & 0x0f;
    const masked = (buf[offset + 1] & 0x80) !== 0;
    let len = buf[offset + 1] & 0x7f;
    let hdr = 2;
    if (len === 126) {
      if (buf.length - offset < 4) break;
      len = buf.readUInt16BE(offset + 2);
      hdr = 4;
    } else if (len === 127) {
      if (buf.length - offset < 10) break;
      len = Number(buf.readBigUInt64BE(offset + 2));
      hdr = 10;
    }
    const maskBytes = masked ? 4 : 0;
    if (buf.length - offset < hdr + maskBytes + len) break;
    const raw = Buffer.from(buf.subarray(offset + hdr + maskBytes, offset + hdr + maskBytes + len));
    if (masked) {
      const mask = buf.subarray(offset + hdr, offset + hdr + 4);
      for (let i = 0; i < raw.length; i += 1) raw[i] ^= mask[i % 4];
    }
    frames.push({ opcode, data: raw });
    offset += hdr + maskBytes + len;
  }
  return { frames, rest: buf.subarray(offset) };
}

export class SessionHub {
  private readonly tabs = new Map<string, ManagedTab>();
  private readonly clients = new Set<net.Socket>();
  private monitorIntervalMs = 2500;

  setMonitorInterval(ms: number): void {
    this.monitorIntervalMs = Math.max(1000, Math.min(10_000, ms));
  }

  handleUpgrade(req: IncomingMessage, sock: net.Socket, expectedToken: string): void {
    const parsedUrl = new URL(req.url ?? '/', 'http://127.0.0.1');
    if (parsedUrl.pathname !== '/ws' || parsedUrl.searchParams.get('token') !== expectedToken) {
      sock.end('HTTP/1.1 401 Unauthorized\r\n\r\n');
      return;
    }
    const key = req.headers['sec-websocket-key'];
    if (typeof key !== 'string' || !key) {
      sock.end('HTTP/1.1 400 Bad Request\r\n\r\n');
      return;
    }
    const accept = createHash('sha1').update(`${key}${WS_MAGIC}`).digest('base64');
    sock.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    this.clients.add(sock);
    let buffer: Buffer = Buffer.alloc(0);
    sock.on('data', (chunk: Buffer) => {
      buffer = Buffer.concat([buffer, chunk]);
      const { frames, rest } = parseClientFrames(buffer);
      buffer = rest;
      for (const f of frames) {
        if (f.opcode === 0x8) {
          this.clients.delete(sock);
          sock.end(encodeWsFrame(0x8, Buffer.alloc(0)));
          return;
        }
        if (f.opcode === 0x9) {
          sock.write(encodeWsFrame(0xa, f.data));
          continue;
        }
        if (f.opcode === 0x1 || f.opcode === 0x0) {
          this.dispatchWsMessage(f.data.toString('utf8'));
        }
      }
    });
    const cleanup = () => this.clients.delete(sock);
    sock.on('close', cleanup);
    sock.on('error', cleanup);
  }

  private dispatchWsMessage(raw: string): void {
    try {
      const msg = JSON.parse(raw) as { type: string; sessionId?: string; sessionIds?: string[]; data?: string; cols?: number; rows?: number };
      if (msg.type === 'term:input' && msg.sessionId && typeof msg.data === 'string') {
        this.tabs.get(msg.sessionId)?.shell.write(msg.data);
      } else if (msg.type === 'term:broadcast' && Array.isArray(msg.sessionIds) && typeof msg.data === 'string') {
        for (const id of msg.sessionIds) this.tabs.get(id)?.shell.write(msg.data);
      } else if (msg.type === 'term:resize' && msg.sessionId && typeof msg.cols === 'number' && typeof msg.rows === 'number') {
        this.tabs.get(msg.sessionId)?.shell.resize(msg.cols, msg.rows);
      }
    } catch {
      // Ignore malformed client control frames safely.
    }
  }

  broadcast(event: Record<string, unknown>): void {
    const frame = encodeWsFrame(0x1, Buffer.from(JSON.stringify(event), 'utf8'));
    for (const sock of this.clients) {
      if (!sock.destroyed) sock.write(frame);
    }
  }

  emitTransfer(task: TransferTask): void {
    this.broadcast({ type: 'transfer:progress', task });
  }

  openLocal(cwd?: string): ConnectInfo {
    const id = randomUUID();
    const shell = openLocalShell({
      cwd,
      onData: (buf) => this.broadcast({ type: 'term:data', sessionId: id, data: buf.toString('base64') }),
      onClose: () => {
        this.tabs.delete(id);
        this.broadcast({ type: 'term:closed', sessionId: id });
      },
    });
    this.tabs.set(id, { id, kind: 'local', alias: '本机终端', shell });
    return { sessionId: id, alias: '本机终端', username: 'local', host: 'localhost', port: 0, kind: 'local' };
  }

  async openSsh(hosts: HostStore, hostId: string, cols: number, rows: number): Promise<ConnectInfo> {
    const chain = resolveHostChain(hosts, hostId);
    if (!chain) throw new Error('找不到这台主机');
    const id = randomUUID();
    const ssh = await connectWithJumps(chain.hops);
    let closed = false;
    const teardown = () => {
      if (closed) return;
      closed = true;
      const tab = this.tabs.get(id);
      if (tab?.monitorTimer) clearInterval(tab.monitorTimer);
      this.tabs.delete(id);
      ssh.dispose();
      this.broadcast({ type: 'term:closed', sessionId: id });
    };

    try {
      const channel = await new Promise<ClientChannel>((resolve, reject) => {
        ssh.client.shell({ term: 'xterm-256color', cols: clampDim(cols, 80), rows: clampDim(rows, 24) }, (err, ch) => {
          if (err || !ch) reject(err ?? new Error('没有拿到终端通道'));
          else resolve(ch);
        });
      });
      channel.on('data', (buf: Buffer) => this.broadcast({ type: 'term:data', sessionId: id, data: buf.toString('base64') }));
      channel.stderr.on('data', (buf: Buffer) => this.broadcast({ type: 'term:data', sessionId: id, data: buf.toString('base64') }));
      channel.on('close', teardown);

      const shell: ShellSession = {
        write: (d: string) => channel.write(d),
        resize: (c: number, r: number) => channel.setWindow(clampDim(r, 24), clampDim(c, 80), 0, 0),
        close: () => {
          channel.close();
          teardown();
        },
      };
      const tab: ManagedTab = { id, kind: 'ssh', alias: chain.target.alias, shell, ssh };
      this.startMetricsLoop(tab);
      this.tabs.set(id, tab);
      return {
        sessionId: id,
        alias: chain.target.alias,
        username: chain.target.username,
        host: chain.target.host,
        port: chain.target.port,
        kind: 'ssh',
      };
    } catch (err) {
      ssh.dispose();
      throw err;
    }
  }

  private startMetricsLoop(tab: ManagedTab): void {
    const poll = async () => {
      if (!tab.ssh || !this.tabs.has(tab.id)) return;
      try {
        const res = await sampleServerMetrics(tab.ssh.client, tab.prevCounters);
        tab.prevCounters = res.counters;
        this.broadcast({ type: 'metrics:update', sessionId: tab.id, metrics: res.metrics });
      } catch {
        // Non-POSIX shells or closed connections skip telemetry without killing shell.
      }
    };
    void poll();
    tab.monitorTimer = setInterval(() => void poll(), this.monitorIntervalMs);
  }

  getSshClient(sessionId: string) {
    const tab = this.tabs.get(sessionId);
    if (!tab || tab.kind !== 'ssh' || !tab.ssh) throw new Error('会话未连接到 SSH 主机');
    return tab.ssh.client;
  }

  closeSession(sessionId: string): void {
    const tab = this.tabs.get(sessionId);
    if (!tab) return;
    if (tab.monitorTimer) clearInterval(tab.monitorTimer);
    tab.shell.close();
  }

  closeAll(): void {
    for (const id of [...this.tabs.keys()]) this.closeSession(id);
    for (const sock of this.clients) sock.destroy();
    this.clients.clear();
  }
}

function clampDim(val: number, fallback: number): number {
  return Number.isInteger(val) && val > 0 ? Math.min(val, 512) : fallback;
}
