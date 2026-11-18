import { execFileSync } from 'node:child_process';
import type { SafeStorage } from './key-file.js';

const PROTECT_SCRIPT = [
  "$ProgressPreference = 'SilentlyContinue'",
  "$b64 = [Console]::In.ReadToEnd().Trim()",
  "if (-not $b64) { throw 'empty input' }",
  'Add-Type -AssemblyName System.Security',
  '$raw = [Convert]::FromBase64String($b64)',
  '$enc = [System.Security.Cryptography.ProtectedData]::Protect($raw, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)',
  '[Console]::Out.Write([Convert]::ToBase64String($enc))',
].join('; ');

const UNPROTECT_SCRIPT = [
  "$ProgressPreference = 'SilentlyContinue'",
  "$b64 = [Console]::In.ReadToEnd().Trim()",
  "if (-not $b64) { throw 'empty input' }",
  'Add-Type -AssemblyName System.Security',
  '$enc = [Convert]::FromBase64String($b64)',
  '$raw = [System.Security.Cryptography.ProtectedData]::Unprotect($enc, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)',
  '[Console]::Out.Write([Convert]::ToBase64String($raw))',
].join('; ');

const PROBE_SCRIPT = "$ProgressPreference = 'SilentlyContinue'; Add-Type -AssemblyName System.Security; [void][System.Security.Cryptography.ProtectedData]";

function encodePsCommand(script: string): string {
  return Buffer.from(script, 'utf16le').toString('base64');
}

function runPs(script: string, stdinText = ''): string {
  return execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodePsCommand(script)], {
    input: stdinText,
    encoding: 'utf8',
    windowsHide: true,
    timeout: 8_000,
  }).trim();
}

/**
 * Pure Node.js SafeStorage adapter backed by Windows DPAPI (CurrentUser) without Electron.
 */
export function createNodeSafeStorage(platform: NodeJS.Platform = process.platform): SafeStorage {
  let cachedAvailable: boolean | null = null;

  return {
    magic: 'TS1D',
    isEncryptionAvailable(): boolean {
      if (platform !== 'win32') return false;
      if (cachedAvailable !== null) return cachedAvailable;
      try {
        runPs(PROBE_SCRIPT);
        cachedAvailable = true;
      } catch {
        cachedAvailable = false;
      }
      return cachedAvailable;
    },
    encryptString(plain: string): Buffer {
      if (!this.isEncryptionAvailable()) {
        throw new Error('当前系统不支持纯 Node DPAPI 密钥环加密');
      }
      const outB64 = runPs(PROTECT_SCRIPT, Buffer.from(plain, 'utf8').toString('base64'));
      return Buffer.from(outB64, 'base64');
    },
    decryptString(encrypted: Buffer): string {
      if (!this.isEncryptionAvailable()) {
        throw new Error('当前系统不支持纯 Node DPAPI 密钥环解密');
      }
      const outB64 = runPs(UNPROTECT_SCRIPT, encrypted.toString('base64'));
      return Buffer.from(outB64, 'base64').toString('utf8');
    },
  };
}
