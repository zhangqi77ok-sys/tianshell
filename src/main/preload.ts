import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import type { ConnectInfo, HostDraft, IpcResult, PrivateKeyPick, PublicHost } from '../shared/types.js';

const api = {
  listHosts: (): Promise<IpcResult<PublicHost[]>> => ipcRenderer.invoke('hosts:list'),
  saveHost: (draft: HostDraft): Promise<IpcResult<PublicHost>> => ipcRenderer.invoke('hosts:save', draft),
  deleteHost: (id: string): Promise<IpcResult<{ id: string }>> => ipcRenderer.invoke('hosts:delete', id),
  pickPrivateKey: (): Promise<IpcResult<PrivateKeyPick | null>> => ipcRenderer.invoke('dialog:privateKey'),
  connect: (hostId: string, cols: number, rows: number): Promise<IpcResult<ConnectInfo>> =>
    ipcRenderer.invoke('ssh:connect', hostId, cols, rows),
  disconnect: (sessionId: string): Promise<IpcResult<{ sessionId: string }>> =>
    ipcRenderer.invoke('ssh:disconnect', sessionId),
  write: (sessionId: string, data: string): void => {
    ipcRenderer.send('ssh:write', sessionId, data);
  },
  resize: (sessionId: string, cols: number, rows: number): void => {
    ipcRenderer.send('ssh:resize', sessionId, cols, rows);
  },
  dataDir: (): Promise<IpcResult<string>> => ipcRenderer.invoke('app:dataDir'),
  onData: (handler: (sessionId: string, data: Uint8Array) => void): (() => void) => {
    const listener = (_event: IpcRendererEvent, sessionId: string, data: Uint8Array) => handler(sessionId, data);
    ipcRenderer.on('ssh:data', listener);
    return () => ipcRenderer.removeListener('ssh:data', listener);
  },
  onClosed: (handler: (sessionId: string) => void): (() => void) => {
    const listener = (_event: IpcRendererEvent, sessionId: string) => handler(sessionId);
    ipcRenderer.on('ssh:closed', listener);
    return () => ipcRenderer.removeListener('ssh:closed', listener);
  },
  onAddHost: (handler: () => void): (() => void) => {
    const listener = () => handler();
    ipcRenderer.on('ui:add-host', listener);
    return () => ipcRenderer.removeListener('ui:add-host', listener);
  },
};

contextBridge.exposeInMainWorld('tianshell', api);

export type TianshellApi = typeof api;
