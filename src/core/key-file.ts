import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import path from 'node:path';

const RAW_MAGIC = Buffer.from('TS1R');
const SAFE_MAGIC = Buffer.from('TS1S');

export interface SafeStorage {
  isEncryptionAvailable(): boolean;
  encryptString(plain: string): Buffer;
  decryptString(encrypted: Buffer): string;
}

export function loadOrCreateKey(filePath: string, safe: SafeStorage | null): Buffer {
  if (existsSync(filePath)) {
    return decodeKeyFile(readFileSync(filePath), safe);
  }
  const key = randomBytes(32);
  const payload = safe?.isEncryptionAvailable() ? encodeSafeKey(key, safe) : Buffer.concat([RAW_MAGIC, key]);
  mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  writeFileSync(filePath, payload, { mode: 0o600 });
  try {
    chmodSync(filePath, 0o600);
  } catch {
    // Windows 上 chmod 不一定能表达“仅当前用户”。系统密钥环可用时，钥匙本身已经包过一层。
  }
  return key;
}

function encodeSafeKey(key: Buffer, safe: SafeStorage): Buffer {
  return Buffer.concat([SAFE_MAGIC, safe.encryptString(key.toString('hex'))]);
}

function decodeKeyFile(buf: Buffer, safe: SafeStorage | null): Buffer {
  const magic = buf.subarray(0, 4);
  if (magic.equals(RAW_MAGIC)) {
    const key = buf.subarray(4);
    if (key.length !== 32) throw new Error('钥匙文件已损坏');
    return Buffer.from(key);
  }
  if (magic.equals(SAFE_MAGIC)) {
    if (!safe?.isEncryptionAvailable()) {
      throw new Error('钥匙放在系统密钥环里，当前进程解不开。请用 node scripts/mcp.mjs 启动 MCP。');
    }
    const key = Buffer.from(safe.decryptString(buf.subarray(4)), 'hex');
    if (key.length !== 32) throw new Error('钥匙文件已损坏');
    return key;
  }
  throw new Error('无法识别的钥匙文件');
}
