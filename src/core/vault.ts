import { createCipheriv, createDecipheriv, randomBytes } from 'node:crypto';

const VERSION = 'v1';

export function seal(key: Buffer, plaintext: string): string {
  if (key.length !== 32) {
    throw new Error('加密钥匙长度不对');
  }
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  const ciphertext = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return [VERSION, iv.toString('base64url'), tag.toString('base64url'), ciphertext.toString('base64url')].join(':');
}

export function open(key: Buffer, sealed: string): string {
  if (key.length !== 32) {
    throw new Error('加密钥匙长度不对');
  }
  const parts = sealed.split(':');
  if (parts.length !== 4 || parts[0] !== VERSION) {
    throw new Error('凭据数据格式不对');
  }
  const iv = Buffer.from(parts[1], 'base64url');
  const tag = Buffer.from(parts[2], 'base64url');
  const ciphertext = Buffer.from(parts[3], 'base64url');
  if (iv.length !== 12 || tag.length !== 16) {
    throw new Error('凭据数据格式不对');
  }
  const decipher = createDecipheriv('aes-256-gcm', key, iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString('utf8');
}
