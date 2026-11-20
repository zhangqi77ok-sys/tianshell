export type AuthType = 'password' | 'privateKey';

export interface HostGroup {
  id: string;
  name: string;
  parentId?: string | null;
  order: number;
}

export interface PublicHost {
  id: string;
  alias: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  hasPassphrase: boolean;
  fingerprint: string | null;
  groupId?: string | null;
  jumpHostId?: string | null;
  tags?: string[];
  notes?: string;
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
  groupId?: string | null;
  jumpHostId?: string | null;
  tags?: string[];
  notes?: string;
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
  kind?: 'ssh' | 'local';
}

export interface PrivateKeyPick {
  name: string;
  text: string;
}

export interface SftpEntry {
  name: string;
  path: string;
  kind: 'dir' | 'file' | 'symlink';
  size: number;
  mtimeMs: number;
  mode: number;
  permissions: string;
  owner?: string;
}

export interface TransferTask {
  id: string;
  sessionId: string;
  direction: 'upload' | 'download';
  filename: string;
  remotePath: string;
  transferred: number;
  total: number;
  status: 'running' | 'done' | 'error' | 'cancelled';
  error?: string;
}

export interface DiskStat {
  filesystem: string;
  mount: string;
  totalKb: number;
  usedKb: number;
  availKb: number;
  usePercent: number;
}

export interface NetStat {
  rxBytesPerSec: number;
  txBytesPerSec: number;
  totalRxBytes: number;
  totalTxBytes: number;
}

export interface DiskIoStat {
  readBytesPerSec: number;
  writeBytesPerSec: number;
  readIops: number;
  writeIops: number;
}

export interface ProcessStat {
  pid: number;
  user: string;
  cpuPercent: number;
  memPercent: number;
  command: string;
}

export interface ServerMetrics {
  sampledAt: string;
  hostname: string;
  uptimeSeconds: number;
  loadAvg: [number, number, number];
  cpuPercent: number;
  memTotalKb: number;
  memUsedKb: number;
  swapTotalKb: number;
  swapUsedKb: number;
  disks: DiskStat[];
  net: NetStat;
  diskIo?: DiskIoStat;
  processes: ProcessStat[];
}

export type TunnelMode = 'local' | 'remote' | 'socks5';

export interface TunnelRule {
  id: string;
  name: string;
  hostId: string;
  mode: TunnelMode;
  localHost: string;
  localPort: number;
  remoteHost?: string;
  remotePort?: number;
}

export interface TunnelRuntime extends TunnelRule {
  active: boolean;
  connections: number;
  error?: string;
}

export interface CommandSnippet {
  id: string;
  name: string;
  category: string;
  command: string;
  description?: string;
}

export interface AppSettings {
  theme: 'dark' | 'light';
  fontSize: number;
  showHiddenFiles: boolean;
  monitorIntervalMs: number;
}

export type IpcResult<T> = { ok: true; data: T } | { ok: false; error: string };
