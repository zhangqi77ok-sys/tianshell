export type AuthType = 'password' | 'privateKey';

export interface PublicHost {
  id: string;
  alias: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  hasPassphrase: boolean;
  fingerprint: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface HostDraft {
  id?: string;
  alias: string;
  host: string;
  port?: number;
  username: string;
  authType: AuthType;
  secret?: string;
  passphrase?: string;
  clearPassphrase?: boolean;
  clearFingerprint?: boolean;
}

export interface ResolvedHost extends PublicHost {
  secret: string;
  passphrase?: string;
}

export interface ConnectInfo {
  sessionId: string;
  alias: string;
  username: string;
  host: string;
  port: number;
}

export interface PrivateKeyPick {
  name: string;
  text: string;
}

export type IpcResult<T> = { ok: true; data: T } | { ok: false; error: string };
