import { randomUUID } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import type { IncomingMessage, ServerResponse } from 'node:http';
import path from 'node:path';
import type { HostStore } from '../core/host-store.js';
import { resolveHostChain } from '../core/host-hops.js';
import type { SettingsStore, SnippetStore, TunnelStore } from '../core/stores.js';
import type { HostDraft, TunnelRule } from '../shared/types.js';
import { explainSshError } from '../ssh/client.js';
import { sampleServerMetrics } from '../ssh/monitor.js';
import {
  chmodRemote,
  downloadRemoteStream,
  listSftpDir,
  mkdirRemote,
  readRemoteText,
  removeRemote,
  renameRemote,
  uploadRemoteStream,
  writeRemoteText,
} from '../ssh/sftp.js';
import type { TunnelManager } from '../ssh/tunnel.js';
import type { SessionHub } from './ws-hub.js';

export interface AppContext {
  hosts: HostStore;
  snippets: SnippetStore;
  tunnels: TunnelStore;
  settings: SettingsStore;
  tunnelMgr: TunnelManager;
  hub: SessionHub;
  token: string;
  distDir: string;
  dataDir: string;
  transfers: Map<string, AbortController>;
}

const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
};

function sendJson(res: ServerResponse, status: number, payload: unknown): void {
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' });
  res.end(JSON.stringify(payload));
}

async function readJsonBody<T>(req: IncomingMessage, maxBytes = 1_048_576): Promise<T> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of req) {
    const b = chunk as Buffer;
    total += b.length;
    if (total > maxBytes) throw new Error('请求体过大');
    chunks.push(b);
  }
  return chunks.length ? (JSON.parse(Buffer.concat(chunks, total).toString('utf8')) as T) : ({} as T);
}

function serveStatic(res: ServerResponse, url: URL, ctx: AppContext): void {
  const rel = url.pathname === '/' ? 'index.html' : url.pathname.replace(/^\/+/, '');
  const resolved = path.resolve(ctx.distDir, rel);
  if (!resolved.startsWith(path.resolve(ctx.distDir)) || !existsSync(resolved)) {
    res.writeHead(404).end('Not found');
    return;
  }
  const ext = path.extname(resolved).toLowerCase();
  res.writeHead(200, { 'content-type': MIME[ext] ?? 'application/octet-stream' });
  if (rel === 'index.html') {
    const html = readFileSync(resolved, 'utf8').replace(
      '</head>',
      `<script>window.__TIANSHELL_TOKEN__=${JSON.stringify(ctx.token)};</script></head>`,
    );
    res.end(html);
    return;
  }
  res.end(readFileSync(resolved));
}

export async function handleHttpRequest(req: IncomingMessage, res: ServerResponse, ctx: AppContext): Promise<void> {
  const url = new URL(req.url ?? '/', 'http://127.0.0.1');
  if (!url.pathname.startsWith('/api/')) {
    serveStatic(res, url, ctx);
    return;
  }

  const hdrToken = req.headers['x-tianshell-token'];
  const qToken = url.searchParams.get('token');
  if (hdrToken !== ctx.token && qToken !== ctx.token) {
    sendJson(res, 401, { ok: false, error: '未授权的本地访问令牌' });
    return;
  }

  try {
    await routeApi(req, res, url, ctx);
  } catch (err) {
    sendJson(res, 400, { ok: false, error: explainSshError(err) });
  }
}

async function routeApi(req: IncomingMessage, res: ServerResponse, url: URL, ctx: AppContext): Promise<void> {
  const m = req.method ?? 'GET';
  const p = url.pathname;

  if (m === 'GET' && p === '/api/hosts') {
    return sendJson(res, 200, { ok: true, data: { hosts: ctx.hosts.list(), groups: ctx.hosts.listGroups(), dataDir: ctx.dataDir } });
  }
  if (m === 'POST' && p === '/api/hosts') return sendJson(res, 200, { ok: true, data: ctx.hosts.save(await readJsonBody<HostDraft>(req)) });
  if (m === 'DELETE' && p.startsWith('/api/hosts/')) {
    ctx.hosts.delete(decodeURIComponent(p.slice('/api/hosts/'.length)));
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && p === '/api/groups') {
    return sendJson(res, 200, { ok: true, data: ctx.hosts.saveGroup(await readJsonBody<{ id?: string; name: string }>(req)) });
  }
  if (m === 'DELETE' && p.startsWith('/api/groups/')) {
    ctx.hosts.deleteGroup(decodeURIComponent(p.slice('/api/groups/'.length)));
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'GET' && p === '/api/snippets') return sendJson(res, 200, { ok: true, data: ctx.snippets.list() });
  if (m === 'POST' && p === '/api/snippets') return sendJson(res, 200, { ok: true, data: ctx.snippets.save(await readJsonBody(req)) });
  if (m === 'DELETE' && p.startsWith('/api/snippets/')) {
    ctx.snippets.delete(decodeURIComponent(p.slice('/api/snippets/'.length)));
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'GET' && p === '/api/tunnels') {
    return sendJson(res, 200, { ok: true, data: ctx.tunnelMgr.getRuntimes(ctx.tunnels.list()) });
  }
  if (m === 'POST' && p === '/api/tunnels') return sendJson(res, 200, { ok: true, data: ctx.tunnels.save(await readJsonBody(req)) });
  if (m === 'DELETE' && p.startsWith('/api/tunnels/')) {
    const id = decodeURIComponent(p.slice('/api/tunnels/'.length));
    await ctx.tunnelMgr.stop(id);
    ctx.tunnels.delete(id);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && p === '/api/tunnels/start') {
    const { id } = await readJsonBody<{ id: string }>(req);
    const rule = ctx.tunnels.list().find((r: TunnelRule) => r.id === id);
    if (!rule) throw new Error('找不到隧道规则');
    const chain = resolveHostChain(ctx.hosts, rule.hostId);
    if (!chain) throw new Error('找不到隧道关联的 SSH 主机');
    const port = await ctx.tunnelMgr.start(rule, chain.hops);
    return sendJson(res, 200, { ok: true, data: { port } });
  }
  if (m === 'POST' && p === '/api/tunnels/stop') {
    await ctx.tunnelMgr.stop((await readJsonBody<{ id: string }>(req)).id);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'GET' && p === '/api/settings') return sendJson(res, 200, { ok: true, data: ctx.settings.get() });
  if (m === 'PUT' && p === '/api/settings') {
    const next = ctx.settings.update(await readJsonBody(req));
    ctx.hub.setMonitorInterval(next.monitorIntervalMs);
    return sendJson(res, 200, { ok: true, data: next });
  }
  if (m === 'POST' && p === '/api/sessions') {
    const body = await readJsonBody<{ kind?: 'ssh' | 'local'; hostId?: string; cols?: number; rows?: number }>(req);
    const info = body.kind === 'local'
      ? ctx.hub.openLocal()
      : await ctx.hub.openSsh(ctx.hosts, body.hostId ?? '', body.cols ?? 80, body.rows ?? 24);
    return sendJson(res, 200, { ok: true, data: info });
  }
  if (m === 'DELETE' && p.startsWith('/api/sessions/')) {
    ctx.hub.closeSession(decodeURIComponent(p.slice('/api/sessions/'.length)));
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && p.startsWith('/api/transfers/') && p.endsWith('/cancel')) {
    const tid = decodeURIComponent(p.slice('/api/transfers/'.length, -'/cancel'.length));
    ctx.transfers.get(tid)?.abort();
    return sendJson(res, 200, { ok: true, data: true });
  }
  await routeSessionOps(req, res, url, ctx);
}

async function routeSessionOps(req: IncomingMessage, res: ServerResponse, url: URL, ctx: AppContext): Promise<void> {
  const m = req.method ?? 'GET';
  const match = /^\/api\/sessions\/([^/]+)\/(.+)$/.exec(url.pathname);
  if (!match) {
    sendJson(res, 404, { ok: false, error: '未知接口路径' });
    return;
  }
  const sid = decodeURIComponent(match[1]);
  const op = match[2];
  const client = ctx.hub.getSshClient(sid);

  if (m === 'GET' && op === 'metrics') {
    return sendJson(res, 200, { ok: true, data: (await sampleServerMetrics(client)).metrics });
  }
  if (m === 'GET' && op === 'sftp/list') {
    return sendJson(res, 200, { ok: true, data: await listSftpDir(client, url.searchParams.get('path') ?? '/') });
  }
  if (m === 'GET' && op === 'sftp/text') {
    return sendJson(res, 200, { ok: true, data: await readRemoteText(client, url.searchParams.get('path') ?? '') });
  }
  if (m === 'PUT' && op === 'sftp/text') {
    const { path: fp, content } = await readJsonBody<{ path: string; content: string }>(req);
    await writeRemoteText(client, fp, content);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && op === 'sftp/mkdir') {
    await mkdirRemote(client, (await readJsonBody<{ path: string }>(req)).path);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && op === 'sftp/rename') {
    const { oldPath, newPath } = await readJsonBody<{ oldPath: string; newPath: string }>(req);
    await renameRemote(client, oldPath, newPath);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && op === 'sftp/chmod') {
    const { path: fp, mode } = await readJsonBody<{ path: string; mode: number }>(req);
    await chmodRemote(client, fp, mode);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'POST' && op === 'sftp/delete') {
    await removeRemote(client, (await readJsonBody<{ path: string }>(req)).path);
    return sendJson(res, 200, { ok: true, data: true });
  }
  if (m === 'GET' && op === 'sftp/download') {
    const remotePath = url.searchParams.get('path') ?? '';
    const tid = randomUUID();
    const ac = new AbortController();
    ctx.transfers.set(tid, ac);
    const fname = path.posix.basename(remotePath) || 'download.bin';
    res.writeHead(200, {
      'content-type': 'application/octet-stream',
      'content-disposition': `attachment; filename*=UTF-8''${encodeURIComponent(fname)}`,
    });
    try {
      await downloadRemoteStream(client, remotePath, res, (tx, total) =>
        ctx.hub.emitTransfer({ id: tid, sessionId: sid, direction: 'download', filename: fname, remotePath, transferred: tx, total, status: 'running' }),
      ac.signal);
      ctx.hub.emitTransfer({ id: tid, sessionId: sid, direction: 'download', filename: fname, remotePath, transferred: 1, total: 1, status: 'done' });
    } catch (e) {
      ctx.hub.emitTransfer({ id: tid, sessionId: sid, direction: 'download', filename: fname, remotePath, transferred: 0, total: 0, status: ac.signal.aborted ? 'cancelled' : 'error', error: explainSshError(e) });
    } finally {
      ctx.transfers.delete(tid);
    }
    return;
  }
  if (m === 'POST' && op === 'sftp/upload') {
    const remotePath = url.searchParams.get('path') ?? '';
    const total = Number(req.headers['content-length'] ?? url.searchParams.get('total') ?? 0) || 0;
    const tid = randomUUID();
    const ac = new AbortController();
    ctx.transfers.set(tid, ac);
    const fname = path.posix.basename(remotePath) || 'upload.bin';
    try {
      await uploadRemoteStream(client, req, remotePath, total, (tx, tot) =>
        ctx.hub.emitTransfer({ id: tid, sessionId: sid, direction: 'upload', filename: fname, remotePath, transferred: tx, total: tot, status: 'running' }),
      ac.signal);
      ctx.hub.emitTransfer({ id: tid, sessionId: sid, direction: 'upload', filename: fname, remotePath, transferred: total, total, status: 'done' });
      sendJson(res, 200, { ok: true, data: { tid } });
    } finally {
      ctx.transfers.delete(tid);
    }
    return;
  }
  sendJson(res, 404, { ok: false, error: '未知 SFTP 接口' });
}
