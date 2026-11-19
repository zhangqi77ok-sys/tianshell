import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTianShellServer } from '../src/web/server.js';

function decodeWsFrames(buf: Buffer): { frames: Array<{ opcode: number; payload: Buffer }>; rest: Buffer } {
  const frames: Array<{ opcode: number; payload: Buffer }> = [];
  let offset = 0;
  while (buf.length - offset >= 2) {
    const opcode = buf[offset] & 0x0f;
    let len = buf[offset + 1] & 0x7f;
    let headerLen = 2;
    if (len === 126) {
      if (buf.length - offset < 4) break;
      len = buf.readUInt16BE(offset + 2);
      headerLen = 4;
    } else if (len === 127) {
      if (buf.length - offset < 10) break;
      len = Number(buf.readBigUInt64BE(offset + 2));
      headerLen = 10;
    }
    if (buf.length - offset < headerLen + len) break;
    frames.push({ opcode, payload: buf.subarray(offset + headerLen, offset + headerLen + len) });
    offset += headerLen + len;
  }
  return { frames, rest: buf.subarray(offset) };
}

function encodeClientTextFrame(text: string): Buffer {
  const data = Buffer.from(text, 'utf8');
  const mask = randomBytes(4);
  const header = data.length < 126
    ? Buffer.from([0x81, 0x80 | data.length])
    : Buffer.from([0x81, 0xfe, (data.length >> 8) & 0xff, data.length & 0xff]);
  const masked = Buffer.alloc(data.length);
  for (let i = 0; i < data.length; i += 1) masked[i] = data[i] ^ mask[i % 4];
  return Buffer.concat([header, mask, masked]);
}

test('HTTP API enforces token auth and WebSocket hub drives live local shell & CRUD', { timeout: 10_000 }, async () => {
  const dataDir = mkdtempSync(path.join(tmpdir(), 'tianshell-web-'));
  const app = await createTianShellServer({ dataDir, port: 0, openBrowser: false, distDir: path.join(process.cwd(), 'dist') });

  try {
    const base = `http://127.0.0.1:${app.port}`;
    const unauth = await fetch(`${base}/api/hosts`);
    assert.equal(unauth.status, 401);

    const headers = { 'content-type': 'application/json', 'x-tianshell-token': app.token };

    // CRUD hosts, groups, snippets, settings
    const grpRes = await fetch(`${base}/api/groups`, { method: 'POST', headers, body: JSON.stringify({ name: '研发机' }) });
    assert.equal(grpRes.status, 200);
    const grp = (await grpRes.json()) as { data: { id: string } };

    const snipRes = await fetch(`${base}/api/snippets`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ name: '负载', category: '监控', command: 'uptime' }),
    });
    assert.equal(snipRes.status, 200);

    const setRes = await fetch(`${base}/api/settings`, { method: 'PUT', headers, body: JSON.stringify({ fontSize: 16 }) });
    assert.equal(setRes.status, 200);
    assert.equal(((await setRes.json()) as { data: { fontSize: number } }).data.fontSize, 16);

    // Connect native WebSocket client
    const wsKey = randomBytes(16).toString('base64');
    const sock = net.connect(app.port, '127.0.0.1');
    await new Promise<void>((resolve, reject) => {
      sock.once('error', reject);
      sock.once('connect', () => {
        sock.write(
          `GET /ws?token=${encodeURIComponent(app.token)} HTTP/1.1\r\n` +
          `Host: 127.0.0.1:${app.port}\r\n` +
          `Upgrade: websocket\r\nConnection: Upgrade\r\n` +
          `Sec-WebSocket-Key: ${wsKey}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
        );
        resolve();
      });
    });

    let wsBuf: Buffer = Buffer.alloc(0);
    let handshakeDone = false;
    let termText = '';
    const expectedAccept = createHash('sha1').update(`${wsKey}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');

    sock.on('data', (chunk: Buffer) => {
      wsBuf = Buffer.concat([wsBuf, chunk]);
      if (!handshakeDone) {
        const endIdx = wsBuf.indexOf('\r\n\r\n');
        if (endIdx === -1) return;
        const head = wsBuf.subarray(0, endIdx).toString('utf8');
        assert.ok(head.includes(expectedAccept));
        handshakeDone = true;
        wsBuf = wsBuf.subarray(endIdx + 4);
      }
      const parsed = decodeWsFrames(wsBuf);
      wsBuf = parsed.rest;
      for (const f of parsed.frames) {
        if (f.opcode !== 0x1) continue;
        const msg = JSON.parse(f.payload.toString('utf8')) as { type: string; data?: string };
        if (msg.type === 'term:data' && msg.data) {
          termText += Buffer.from(msg.data, 'base64').toString('utf8');
        }
      }
    });

    while (!handshakeDone) await new Promise((r) => setTimeout(r, 20));

    // Open local shell session
    const sessRes = await fetch(`${base}/api/sessions`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ kind: 'local', groupId: grp.data.id }),
    });
    assert.equal(sessRes.status, 200);
    const sessId = ((await sessRes.json()) as { data: { sessionId: string } }).data.sessionId;

    sock.write(encodeClientTextFrame(JSON.stringify({ type: 'term:input', sessionId: sessId, data: 'echo TIAN_WS_OK\n' })));
    const deadline = Date.now() + 5000;
    while (!termText.includes('TIAN_WS_OK') && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 40));
    }
    assert.match(termText, /TIAN_WS_OK/);

    await fetch(`${base}/api/sessions/${encodeURIComponent(sessId)}`, { method: 'DELETE', headers });
    sock.destroy();
  } finally {
    await app.close();
    rmSync(dataDir, { recursive: true, force: true });
  }
});
