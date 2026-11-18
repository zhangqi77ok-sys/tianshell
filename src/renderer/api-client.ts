import type {
  AppSettings,
  CommandSnippet,
  ConnectInfo,
  HostDraft,
  HostGroup,
  IpcResult,
  PublicHost,
  SftpEntry,
  TunnelRule,
  TunnelRuntime,
} from '../shared/types.js';

declare global {
  interface Window {
    __TIANSHELL_TOKEN__?: string;
  }
}

export function resolveClientToken(): string {
  if (typeof window !== 'undefined') {
    if (window.__TIANSHELL_TOKEN__) return window.__TIANSHELL_TOKEN__;
    const q = new URLSearchParams(window.location.search).get('token');
    if (q) return q;
  }
  return '';
}

export async function apiRequest<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = resolveClientToken();
  const headers = new Headers(init.headers);
  if (token) headers.set('x-tianshell-token', token);
  if (init.body && typeof init.body === 'string' && !headers.has('content-type')) {
    headers.set('content-type', 'application/json');
  }
  const resp = await fetch(path, { ...init, headers });
  const json = (await resp.json()) as IpcResult<T>;
  if (!json.ok) throw new Error(json.error || `请求失败 (${resp.status})`);
  return json.data;
}

export const webApi = {
  getHosts: () => apiRequest<{ hosts: PublicHost[]; groups: HostGroup[]; dataDir: string }>('/api/hosts'),
  saveHost: (draft: HostDraft) => apiRequest<PublicHost>('/api/hosts', { method: 'POST', body: JSON.stringify(draft) }),
  deleteHost: (id: string) => apiRequest<boolean>(`/api/hosts/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  saveGroup: (name: string, id?: string) => apiRequest<HostGroup>('/api/groups', { method: 'POST', body: JSON.stringify({ name, id }) }),
  deleteGroup: (id: string) => apiRequest<boolean>(`/api/groups/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  listSnippets: () => apiRequest<CommandSnippet[]>('/api/snippets'),
  saveSnippet: (draft: Omit<CommandSnippet, 'id'> & { id?: string }) =>
    apiRequest<CommandSnippet>('/api/snippets', { method: 'POST', body: JSON.stringify(draft) }),
  deleteSnippet: (id: string) => apiRequest<boolean>(`/api/snippets/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  listTunnels: () => apiRequest<TunnelRuntime[]>('/api/tunnels'),
  saveTunnel: (rule: Omit<TunnelRule, 'id'> & { id?: string }) =>
    apiRequest<TunnelRule>('/api/tunnels', { method: 'POST', body: JSON.stringify(rule) }),
  deleteTunnel: (id: string) => apiRequest<boolean>(`/api/tunnels/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  startTunnel: (id: string) => apiRequest<{ port: number }>('/api/tunnels/start', { method: 'POST', body: JSON.stringify({ id }) }),
  stopTunnel: (id: string) => apiRequest<boolean>('/api/tunnels/stop', { method: 'POST', body: JSON.stringify({ id }) }),

  getSettings: () => apiRequest<AppSettings>('/api/settings'),
  updateSettings: (patch: Partial<AppSettings>) =>
    apiRequest<AppSettings>('/api/settings', { method: 'PUT', body: JSON.stringify(patch) }),

  openSession: (payload: { kind: 'ssh' | 'local'; hostId?: string; cols?: number; rows?: number }) =>
    apiRequest<ConnectInfo>('/api/sessions', { method: 'POST', body: JSON.stringify(payload) }),
  closeSession: (sessionId: string) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sessionId)}`, { method: 'DELETE' }),

  sftpList: (sid: string, dirPath: string) =>
    apiRequest<SftpEntry[]>(`/api/sessions/${encodeURIComponent(sid)}/sftp/list?path=${encodeURIComponent(dirPath)}`),
  sftpReadText: (sid: string, filePath: string) =>
    apiRequest<{ content: string; truncated: boolean }>(`/api/sessions/${encodeURIComponent(sid)}/sftp/text?path=${encodeURIComponent(filePath)}`),
  sftpWriteText: (sid: string, filePath: string, content: string) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sid)}/sftp/text`, {
      method: 'PUT',
      body: JSON.stringify({ path: filePath, content }),
    }),
  sftpMkdir: (sid: string, dirPath: string) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sid)}/sftp/mkdir`, { method: 'POST', body: JSON.stringify({ path: dirPath }) }),
  sftpRename: (sid: string, oldPath: string, newPath: string) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sid)}/sftp/rename`, {
      method: 'POST',
      body: JSON.stringify({ oldPath, newPath }),
    }),
  sftpDelete: (sid: string, targetPath: string) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sid)}/sftp/delete`, { method: 'POST', body: JSON.stringify({ path: targetPath }) }),
  sftpChmod: (sid: string, targetPath: string, mode: number) =>
    apiRequest<boolean>(`/api/sessions/${encodeURIComponent(sid)}/sftp/chmod`, {
      method: 'POST',
      body: JSON.stringify({ path: targetPath, mode }),
    }),
  sftpDownloadUrl: (sid: string, targetPath: string) =>
    `/api/sessions/${encodeURIComponent(sid)}/sftp/download?path=${encodeURIComponent(targetPath)}&token=${encodeURIComponent(resolveClientToken())}`,
  sftpUploadFile: async (sid: string, targetPath: string, file: File) =>
    apiRequest<{ tid: string }>(
      `/api/sessions/${encodeURIComponent(sid)}/sftp/upload?path=${encodeURIComponent(targetPath)}&total=${file.size}`,
      { method: 'POST', body: file },
    ),
  cancelTransfer: (tid: string) =>
    apiRequest<boolean>(`/api/transfers/${encodeURIComponent(tid)}/cancel`, { method: 'POST' }),
};

export function connectWs(onEvent: (msg: Record<string, unknown>) => void): WebSocket {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${proto}//${location.host}/ws?token=${encodeURIComponent(resolveClientToken())}`);
  ws.addEventListener('message', (ev) => {
    try {
      onEvent(JSON.parse(String(ev.data)) as Record<string, unknown>);
    } catch {
      // Ignore malformed server events.
    }
  });
  return ws;
}
