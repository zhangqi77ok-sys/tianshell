import { createRequire } from 'node:module';
import path from 'node:path';
import type { Readable, Writable } from 'node:stream';
import type { SFTPWrapper } from 'ssh2';
import type { SftpEntry } from '../shared/types.js';

const require = createRequire(import.meta.url);
const { Client: SshClient } = require('ssh2') as typeof import('ssh2');
type SshClientInstance = InstanceType<typeof SshClient>;

export function normalizeRemotePath(base: string, target = ''): string {
  const cleanBase = base?.trim() || '/';
  const raw = target.trim();
  if (!raw) return path.posix.normalize(cleanBase.startsWith('/') ? cleanBase : `/${cleanBase}`);
  return raw.startsWith('/') ? path.posix.normalize(raw) : path.posix.resolve(cleanBase.startsWith('/') ? cleanBase : `/${cleanBase}`, raw);
}

export function formatUnixMode(mode: number): string {
  const ftype = (mode & 0o170000) === 0o040000 ? 'd' : (mode & 0o170000) === 0o120000 ? 'l' : '-';
  const perms = [0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001]
    .map((bit, idx) => (mode & bit ? 'rwx'[idx % 3] : '-'))
    .join('');
  return `${ftype}${perms}`;
}

export async function withSftp<T>(client: SshClientInstance, fn: (sftp: SFTPWrapper) => Promise<T>): Promise<T> {
  const sftp = await new Promise<SFTPWrapper>((resolve, reject) => {
    client.sftp((err, wrapper) => {
      if (err) reject(err);
      else resolve(wrapper);
    });
  });
  try {
    return await fn(sftp);
  } finally {
    try {
      sftp.end();
    } catch {
      // Ignore already closed SFTP channels.
    }
  }
}

export async function listSftpDir(client: SshClientInstance, dirPath: string): Promise<SftpEntry[]> {
  const target = normalizeRemotePath(dirPath);
  return withSftp(client, async (sftp) => {
    const raw = await new Promise<import('ssh2').FileEntry[]>((resolve, reject) => {
      sftp.readdir(target, (err, list) => (err ? reject(err) : resolve(list)));
    });
    const entries: SftpEntry[] = [];
    for (const item of raw) {
      if (item.filename === '.' || item.filename === '..') continue;
      const mode = item.attrs?.mode ?? 0;
      const kind: SftpEntry['kind'] =
        (mode & 0o170000) === 0o040000 ? 'dir' : (mode & 0o170000) === 0o120000 ? 'symlink' : 'file';
      entries.push({
        name: item.filename,
        path: path.posix.join(target, item.filename),
        kind,
        size: Number(item.attrs?.size ?? 0),
        mtimeMs: Number(item.attrs?.mtime ?? 0) * 1000,
        mode: mode & 0o777,
        permissions: formatUnixMode(mode),
      });
    }
    return entries.sort((a, b) => {
      if (a.kind === 'dir' && b.kind !== 'dir') return -1;
      if (a.kind !== 'dir' && b.kind === 'dir') return 1;
      return a.name.localeCompare(b.name, 'zh-CN');
    });
  });
}

export async function mkdirRemote(client: SshClientInstance, remotePath: string): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  await withSftp(client, (sftp) => new Promise<void>((res, rej) => sftp.mkdir(target, (e) => (e ? rej(e) : res()))));
}

export async function renameRemote(client: SshClientInstance, oldPath: string, newPath: string): Promise<void> {
  const src = normalizeRemotePath(oldPath);
  const dst = normalizeRemotePath(newPath);
  await withSftp(client, (sftp) => new Promise<void>((res, rej) => sftp.rename(src, dst, (e) => (e ? rej(e) : res()))));
}

export async function chmodRemote(client: SshClientInstance, remotePath: string, mode: number): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  await withSftp(client, (sftp) => new Promise<void>((res, rej) => sftp.chmod(target, mode & 0o777, (e) => (e ? rej(e) : res()))));
}

async function removeRecursive(sftp: SFTPWrapper, target: string): Promise<void> {
  const stat = await new Promise<import('ssh2').Stats>((res, rej) => sftp.lstat(target, (e, st) => (e ? rej(e) : res(st))));
  if (!stat.isDirectory()) {
    await new Promise<void>((res, rej) => sftp.unlink(target, (e) => (e ? rej(e) : res())));
    return;
  }
  const children = await new Promise<import('ssh2').FileEntry[]>((res, rej) =>
    sftp.readdir(target, (e, list) => (e ? rej(e) : res(list))),
  );
  for (const child of children) {
    if (child.filename === '.' || child.filename === '..') continue;
    await removeRecursive(sftp, path.posix.join(target, child.filename));
  }
  await new Promise<void>((res, rej) => sftp.rmdir(target, (e) => (e ? rej(e) : res())));
}

export async function removeRemote(client: SshClientInstance, remotePath: string): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  if (target === '/') throw new Error('安全拦截：禁止删除根目录 /');
  await withSftp(client, (sftp) => removeRecursive(sftp, target));
}

export async function readRemoteText(
  client: SshClientInstance,
  remotePath: string,
  maxBytes = 256 * 1024,
): Promise<{ content: string; truncated: boolean }> {
  const target = normalizeRemotePath(remotePath);
  return withSftp(client, async (sftp) => {
    const buf = await new Promise<Buffer>((resolve, reject) => {
      const chunks: Buffer[] = [];
      let total = 0;
      const rs = sftp.createReadStream(target, { start: 0, end: maxBytes });
      rs.on('data', (chunk: Buffer) => {
        chunks.push(chunk);
        total += chunk.length;
      });
      rs.once('error', reject);
      rs.once('end', () => resolve(Buffer.concat(chunks, total)));
    });
    if (buf.subarray(0, 1024).includes(0)) {
      throw new Error('检测到二进制文件，暂不支持在线文本编辑');
    }
    const truncated = buf.length > maxBytes;
    const sliced = truncated ? buf.subarray(0, maxBytes) : buf;
    return { content: sliced.toString('utf8'), truncated };
  });
}

export async function writeRemoteText(client: SshClientInstance, remotePath: string, content: string): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  await withSftp(
    client,
    (sftp) =>
      new Promise<void>((resolve, reject) => {
        const ws = sftp.createWriteStream(target, { encoding: 'utf8', flags: 'w' });
        ws.once('error', reject);
        ws.once('close', resolve);
        ws.end(content);
      }),
  );
}

export async function uploadRemoteStream(
  client: SshClientInstance,
  source: Readable,
  remotePath: string,
  totalBytes = 0,
  onProgress?: (transferred: number, total: number) => void,
  signal?: AbortSignal,
): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  await withSftp(
    client,
    (sftp) =>
      new Promise<void>((resolve, reject) => {
        let done = false;
        let transferred = 0;
        const ws = sftp.createWriteStream(target, { flags: 'w' });
        const cleanup = (err?: unknown) => {
          if (done) return;
          done = true;
          source.destroy();
          ws.destroy();
          if (err) reject(err);
          else resolve();
        };
        if (signal?.aborted) return cleanup(new Error('上传已取消'));
        signal?.addEventListener('abort', () => cleanup(new Error('上传已取消')), { once: true });
        source.on('data', (chunk: Buffer) => {
          transferred += chunk.length;
          onProgress?.(transferred, totalBytes);
        });
        source.once('error', cleanup);
        ws.once('error', cleanup);
        ws.once('close', () => cleanup());
        source.pipe(ws);
      }),
  );
}

export async function downloadRemoteStream(
  client: SshClientInstance,
  remotePath: string,
  dest: Writable,
  onProgress?: (transferred: number, total: number) => void,
  signal?: AbortSignal,
): Promise<void> {
  const target = normalizeRemotePath(remotePath);
  await withSftp(client, async (sftp) => {
    const stat = await new Promise<import('ssh2').Stats>((res, rej) => sftp.stat(target, (e, st) => (e ? rej(e) : res(st))));
    const total = Number(stat.size ?? 0);
    await new Promise<void>((resolve, reject) => {
      let done = false;
      let transferred = 0;
      const rs = sftp.createReadStream(target);
      const cleanup = (err?: unknown) => {
        if (done) return;
        done = true;
        rs.destroy();
        if (err) reject(err);
        else resolve();
      };
      if (signal?.aborted) return cleanup(new Error('下载已取消'));
      signal?.addEventListener('abort', () => cleanup(new Error('下载已取消')), { once: true });
      rs.on('data', (chunk: Buffer) => {
        transferred += chunk.length;
        onProgress?.(transferred, total);
      });
      rs.once('error', cleanup);
      dest.once('error', cleanup);
      dest.once('finish', () => cleanup());
      rs.pipe(dest);
    });
  });
}
