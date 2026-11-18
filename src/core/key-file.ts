import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import path from 'node:path';

const RAW_MAGIC = Buffer.from('TS1R');
const SAFE_MAGIC = Buffer.from('TS1S');
const DPAPI_MAGIC = Buffer.from('TS1D');

export interface SafeStorage {
  readonly magic?: string;
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
  const magic = Buffer.from(safe.magic ?? 'TS1S', 'utf8');
  if (magic.length !== 4) {
    throw new Error('不支持的密钥封装标识');
  }
  return Buffer.concat([magic, safe.encryptString(key.toString('hex'))]);
}

function decodeHexKey(hex: string): Buffer {
  const key = Buffer.from(hex.trim(), 'hex');
  if (key.length !== 32) throw new Error('钥匙文件已损坏');
  return key;
}

function decodeKeyFile(buf: Buffer, safe: SafeStorage | null): Buffer {
  const magic = buf.subarray(0, 4);
  if (magic.equals(RAW_MAGIC)) {
    const key = buf.subarray(4);
    if (key.length !== 32) throw new Error('钥匙文件已损坏');
    return Buffer.from(key);
  }
  if (magic.equals(DPAPI_MAGIC)) {
    if (!safe || safe.magic !== 'TS1D' || !safe.isEncryptionAvailable()) {
      throw new Error('钥匙由 Windows DPAPI 系统密钥环保护，当前环境无法解开。');
    }
    return decodeHexKey(safe.decryptString(buf.subarray(4)));
  }
  if (magic.equals(SAFE_MAGIC)) {
    if (safe?.magic === 'TS1D') {
      throw new Error('钥匙由旧版 Electron 系统密钥环 (TS1S) 加密，纯 Node 模式无法解开；请迁移或清除旧 vault.key / hosts.json。');
    }
    if (!safe?.isEncryptionAvailable()) {
      throw new Error('钥匙放在系统密钥环里，当前进程解不开。');
    }
    return decodeHexKey(safe.decryptString(buf.subarray(4)));
  }
  throw new Error('无法识别的钥匙文件');
}
