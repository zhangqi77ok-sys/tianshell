import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import {
  chmodSync,
  closeSync,
  constants,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  readSync,
  renameSync,
  rmdirSync,
  rmSync,
  statSync,
  unlinkSync,
  writeSync,
} from 'node:fs';
import net from 'node:net';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { connectWithJumps } from '../src/ssh/client.js';
import {
  chmodRemote,
  downloadRemoteStream,
  formatUnixMode,
  listSftpDir,
  mkdirRemote,
  normalizeRemotePath,
  readRemoteText,
  removeRemote,
  renameRemote,
  uploadRemoteStream,
  writeRemoteText,
} from '../src/ssh/sftp.js';

const require = createRequire(import.meta.url);
const { Server, utils: sshUtils } = require('ssh2') as typeof import('ssh2');
const STATUS = sshUtils.sftp.STATUS_CODE;
const OPEN = sshUtils.sftp.OPEN_MODE;

function makeHostKey(): string {
  const dir = mkdtempSync(path.join(tmpdir(), 'tianshell-sftp-k-'));
  const file = path.join(dir, 'k');
  execFileSync('ssh-keygen', ['-t', 'ed25519', '-f', file, '-N', '', '-q']);
  const k = readFileSync(file, 'utf8');
  rmSync(dir, { recursive: true, force: true });
  return k;
}

function toFs(root: string, remotePath: string): string {
  const norm = path.posix.normalize(`/${remotePath}`).replace(/^\/+/, '');
  return path.join(root, ...norm.split('/').filter(Boolean));
}

async function startTestSftpServer(root: string): Promise<{ port: number; close: () => Promise<void> }> {
  const server = new Server({ hostKeys: [makeHostKey()] }, (client) => {
    client.on('authentication', (ctx) => ctx.accept());
    client.on('ready', () => {
      client.on('session', (accept) => {
        const s = accept();
        s.on('sftp', (acceptSftp) => {
          const sftp = acceptSftp();
          type Handle = { kind: 'dir'; fsPath: string; items: string[]; sent: boolean } | { kind: 'file'; fd: number };
          const handles = new Map<number, Handle>();
          let nextId = 1;
          const attrsOf = (st: import('node:fs').Stats) => ({
            mode: Number(st.mode),
            uid: 1000,
            gid: 1000,
            size: Number(st.size),
            atime: Math.floor(Number(st.atimeMs) / 1000),
            mtime: Math.floor(Number(st.mtimeMs) / 1000),
          });

          sftp.on('OPENDIR', (reqId, rp) => {
            try {
              const fsPath = toFs(root, rp);
              const id = nextId++;
              handles.set(id, { kind: 'dir', fsPath, items: readdirSync(fsPath), sent: false });
              sftp.handle(reqId, Buffer.from(String(id)));
            } catch { sftp.status(reqId, STATUS.NO_SUCH_FILE); }
          });
          sftp.on('READDIR', (reqId, hBuf) => {
            const h = handles.get(Number(hBuf.toString()));
            if (!h || h.kind !== 'dir' || h.sent) return sftp.status(reqId, STATUS.EOF);
            h.sent = true;
            sftp.name(reqId, h.items.map((name) => {
              const st = statSync(path.join(h.fsPath, name));
              return { filename: name, longname: name, attrs: attrsOf(st) };
            }));
          });
          sftp.on('STAT', (reqId, rp) => {
            try { sftp.attrs(reqId, attrsOf(statSync(toFs(root, rp)))); }
            catch { sftp.status(reqId, STATUS.NO_SUCH_FILE); }
          });
          sftp.on('LSTAT', (reqId, rp) => {
            try { sftp.attrs(reqId, attrsOf(statSync(toFs(root, rp)))); }
            catch { sftp.status(reqId, STATUS.NO_SUCH_FILE); }
          });
          sftp.on('OPEN', (reqId, rp, flags) => {
            try {
              let nodeFlags = 0;
              if ((flags & OPEN.WRITE) && (flags & OPEN.READ)) nodeFlags = constants.O_RDWR;
              else if (flags & OPEN.WRITE) nodeFlags = constants.O_WRONLY;
              else nodeFlags = constants.O_RDONLY;
              if (flags & OPEN.CREAT) nodeFlags |= constants.O_CREAT;
              if (flags & OPEN.TRUNC) nodeFlags |= constants.O_TRUNC;
              const fd = openSync(toFs(root, rp), nodeFlags, 0o644);
              const id = nextId++;
              handles.set(id, { kind: 'file', fd });
              sftp.handle(reqId, Buffer.from(String(id)));
            } catch { sftp.status(reqId, STATUS.FAILURE); }
          });
          sftp.on('READ', (reqId, hBuf, offset, len) => {
            const h = handles.get(Number(hBuf.toString()));
            if (!h || h.kind !== 'file') return sftp.status(reqId, STATUS.FAILURE);
            const buf = Buffer.alloc(len);
            const n = readSync(h.fd, buf, 0, len, offset);
            if (n === 0) sftp.status(reqId, STATUS.EOF);
            else sftp.data(reqId, buf.subarray(0, n));
          });
          sftp.on('WRITE', (reqId, hBuf, offset, data) => {
            const h = handles.get(Number(hBuf.toString()));
            if (!h || h.kind !== 'file') return sftp.status(reqId, STATUS.FAILURE);
            writeSync(h.fd, data, 0, data.length, offset);
            sftp.status(reqId, STATUS.OK);
          });
          sftp.on('MKDIR', (reqId, rp) => {
            try { mkdirSync(toFs(root, rp)); sftp.status(reqId, STATUS.OK); }
            catch { sftp.status(reqId, STATUS.FAILURE); }
          });
          sftp.on('RENAME', (reqId, oldP, newP) => {
            try { renameSync(toFs(root, oldP), toFs(root, newP)); sftp.status(reqId, STATUS.OK); }
            catch { sftp.status(reqId, STATUS.FAILURE); }
          });
          sftp.on('REMOVE', (reqId, rp) => {
            try { unlinkSync(toFs(root, rp)); sftp.status(reqId, STATUS.OK); }
            catch { sftp.status(reqId, STATUS.NO_SUCH_FILE); }
          });
          sftp.on('RMDIR', (reqId, rp) => {
            try { rmdirSync(toFs(root, rp)); sftp.status(reqId, STATUS.OK); }
            catch { sftp.status(reqId, STATUS.FAILURE); }
          });
          sftp.on('SETSTAT', (reqId, rp, attrs) => {
            try {
              if (attrs.mode !== undefined) chmodSync(toFs(root, rp), attrs.mode & 0o777);
              sftp.status(reqId, STATUS.OK);
            } catch { sftp.status(reqId, STATUS.FAILURE); }
          });
          sftp.on('CLOSE', (reqId, hBuf) => {
            const id = Number(hBuf.toString());
            const h = handles.get(id);
            if (h && h.kind === 'file') closeSync(h.fd);
            handles.delete(id);
            sftp.status(reqId, STATUS.OK);
          });
        });
      });
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  return {
    port: (server.address() as net.AddressInfo).port,
    close: () => new Promise((r) => server.close(() => r())),
  };
}

test('formatUnixMode and normalizeRemotePath handle permissions and posix paths cleanly', () => {
  assert.equal(formatUnixMode(0o100644), '-rw-r--r--');
  assert.equal(formatUnixMode(0o040755), 'drwxr-xr-x');
  assert.equal(normalizeRemotePath('/var/www/app', '../logs/out.log'), '/var/www/logs/out.log');
});

test('SFTP client supports full directory, text editor, chmod, recursive delete, and streaming transfer', { timeout: 10_000 }, async () => {
  const root = mkdtempSync(path.join(tmpdir(), 'tianshell-sftp-root-'));
  const srv = await startTestSftpServer(root);
  try {
    const conn = await connectWithJumps([
      { target: { host: '127.0.0.1', port: srv.port, username: 'u', authType: 'password', secret: 'p' }, acceptHostKey: () => true },
    ]);
    try {
      await mkdirRemote(conn.client, '/app');
      await writeRemoteText(conn.client, '/app/nginx.conf', 'server { listen 80; }\n');

      const entries = await listSftpDir(conn.client, '/app');
      assert.equal(entries.length, 1);
      assert.equal(entries[0]?.name, 'nginx.conf');
      assert.equal(entries[0]?.kind, 'file');

      const readRes = await readRemoteText(conn.client, '/app/nginx.conf');
      assert.equal(readRes.truncated, false);
      assert.match(readRes.content, /listen 80/);

      await chmodRemote(conn.client, '/app/nginx.conf', 0o750);
      await renameRemote(conn.client, '/app/nginx.conf', '/app/site.conf');

      // Upload & download stream with progress callback
      const upBytes = Buffer.from('hello-stream-upload-12345', 'utf8');
      const upProgress: number[] = [];
      const src = new PassThrough();
      src.end(upBytes);
      await uploadRemoteStream(conn.client, src, '/app/blob.bin', upBytes.length, (n) => upProgress.push(n));
      assert.ok(upProgress.some((n) => n === upBytes.length));

      const chunks: Buffer[] = [];
      const dest = new PassThrough();
      dest.on('data', (c: Buffer) => chunks.push(c));
      await downloadRemoteStream(conn.client, '/app/blob.bin', dest);
      assert.equal(Buffer.concat(chunks).toString('utf8'), 'hello-stream-upload-12345');

      // Recursive remove directory and files inside
      await removeRemote(conn.client, '/app');
      const rootEntries = await listSftpDir(conn.client, '/');
      assert.equal(rootEntries.length, 0);
    } finally {
      conn.dispose();
    }
  } finally {
    await srv.close();
    rmSync(root, { recursive: true, force: true });
  }
});
