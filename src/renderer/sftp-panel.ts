import type { SftpEntry, TransferTask } from '../shared/types.js';
import { webApi } from './api-client.js';

export interface SftpControllerDeps {
  getActiveSshId: () => string | null;
  toast: (msg: string) => void;
  promptText: (title: string, defaultVal?: string) => Promise<string | null>;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(1)} ${units[u]}`;
}

export class SftpController {
  private currentDir = '/';
  private showHidden = false;
  private lastSig = '';
  private readonly transfers = new Map<string, TransferTask>();
  private editingPath: string | null = null;

  private readonly pathInput = document.getElementById('sftp-path') as HTMLInputElement;
  private readonly bodyEl = document.getElementById('sftp-body') as HTMLTableSectionElement;
  private readonly tableEl = document.getElementById('sftp-table') as HTMLTableElement;
  private readonly emptyEl = document.getElementById('sftp-empty') as HTMLDivElement;
  private readonly wrapEl = document.getElementById('sftp-table-wrap') as HTMLDivElement;
  private readonly txListEl = document.getElementById('transfer-list') as HTMLDivElement;
  private readonly txSumEl = document.getElementById('transfer-summary') as HTMLSpanElement;
  private readonly editorDlg = document.getElementById('editor-dialog') as HTMLDialogElement;
  private readonly editorContent = document.getElementById('editor-content') as HTMLTextAreaElement;
  private readonly editorPathEl = document.getElementById('editor-path') as HTMLParagraphElement;

  constructor(private readonly deps: SftpControllerDeps) {
    this.bindEvents();
    window.setInterval(() => {
      if (this.deps.getActiveSshId()) void this.load(this.currentDir, true);
    }, 3000);
  }

  setShowHidden(flag: boolean): void {
    this.showHidden = flag;
    if (this.deps.getActiveSshId()) void this.load(this.currentDir);
  }

  syncSession(): void {
    this.lastSig = '';
    const active = Boolean(this.deps.getActiveSshId());
    for (const id of ['sftp-up', 'sftp-home', 'sftp-path', 'sftp-go', 'sftp-mkdir', 'sftp-upload', 'sftp-refresh']) {
      const el = document.getElementById(id) as HTMLButtonElement | HTMLInputElement;
      el.disabled = !active;
    }
    if (active) {
      void this.load(this.currentDir, true);
    } else {
      this.lastSig = '';
      this.tableEl.classList.add('hidden');
      this.emptyEl.classList.remove('hidden');
      this.emptyEl.textContent = '空闲。连上 SSH 会话后，这里显示该会话的当前目录。';
      this.bodyEl.textContent = '';
    }
  }

  async load(dirPath: string, quiet = false): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    try {
      const list = await webApi.sftpList(sid, dirPath);
      this.currentDir = dirPath || '/';
      this.pathInput.value = this.currentDir;
      const visible = list.filter((e) => this.showHidden || !e.name.startsWith('.'));
      const sig = `${this.currentDir}|${visible.map((e) => `${e.path}:${e.size}:${e.mtimeMs}:${e.mode}`).join('|')}`;
      if (sig === this.lastSig) return;
      this.lastSig = sig;
      this.renderEntries(visible);
    } catch (err) {
      if (!quiet) this.deps.toast(err instanceof Error ? err.message : 'SFTP 读取失败');
    }
  }

  onTransferProgress(task: TransferTask): void {
    this.transfers.set(task.id, task);
    this.renderTransfers();
    if (task.status === 'done' && task.direction === 'upload') void this.load(this.currentDir);
  }

  private renderEntries(entries: SftpEntry[]): void {
    this.bodyEl.textContent = '';
    this.tableEl.classList.remove('hidden');
    this.emptyEl.classList.add('hidden');
    for (const entry of entries) {
      const tr = document.createElement('tr');
      const nameTd = document.createElement('td');
      const nameBtn = document.createElement('span');
      nameBtn.textContent = `${entry.kind === 'dir' ? '📁 ' : '📄 '}${entry.name}`;
      nameBtn.style.cursor = 'pointer';
      nameTd.appendChild(nameBtn);
      tr.appendChild(nameTd);

      for (const val of [entry.permissions, entry.kind === 'dir' ? '--' : formatBytes(entry.size), entry.mtimeMs ? new Date(entry.mtimeMs).toLocaleString() : '--']) {
        const td = document.createElement('td');
        td.className = 'mono';
        td.textContent = val;
        tr.appendChild(td);
      }

      const opsTd = document.createElement('td');
      opsTd.style.display = 'flex';
      opsTd.style.gap = '4px';
      if (entry.kind === 'file') {
        this.addOpBtn(opsTd, '编辑', () => void this.openEditor(entry.path));
        this.addOpBtn(opsTd, '下载', () => this.triggerDownload(entry.path));
      }
      this.addOpBtn(opsTd, '改名', () => void this.renameItem(entry));
      this.addOpBtn(opsTd, '权限', () => void this.chmodItem(entry));
      this.addOpBtn(opsTd, '删', () => void this.deleteItem(entry));
      tr.appendChild(opsTd);

      tr.addEventListener('dblclick', () => {
        if (entry.kind === 'dir') void this.load(entry.path);
        else if (entry.kind === 'file') void this.openEditor(entry.path);
      });
      this.bodyEl.appendChild(tr);
    }
  }

  private addOpBtn(parent: HTMLElement, label: string, onClick: () => void): void {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = label;
    b.style.padding = '2px 6px';
    b.style.fontSize = '11px';
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      onClick();
    });
    parent.appendChild(b);
  }

  private async openEditor(remotePath: string): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    try {
      const res = await webApi.sftpReadText(sid, remotePath);
      this.editingPath = remotePath;
      this.editorPathEl.textContent = res.truncated ? `${remotePath}（仅预览前 256KB）` : remotePath;
      this.editorContent.value = res.content;
      this.editorDlg.showModal();
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '读取文件失败');
    }
  }

  private triggerDownload(remotePath: string): void {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const a = document.createElement('a');
    a.href = webApi.sftpDownloadUrl(sid, remotePath);
    a.download = remotePath.split('/').pop() || 'file';
    document.body.appendChild(a);
    a.click();
    a.remove();
  }

  private async renameItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const nextName = await this.deps.promptText(`重命名 ${entry.name}`, entry.name);
    if (!nextName || nextName === entry.name) return;
    const parent = this.currentDir === '/' ? '' : this.currentDir;
    await webApi.sftpRename(sid, entry.path, `${parent}/${nextName}`);
    await this.load(this.currentDir);
  }

  private async chmodItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const raw = await this.deps.promptText('输入八进制权限 (例如 755 或 644)', entry.mode.toString(8));
    if (!raw) return;
    const mode = Number.parseInt(raw.trim(), 8);
    if (Number.isNaN(mode)) return this.deps.toast('权限格式无效');
    await webApi.sftpChmod(sid, entry.path, mode);
    await this.load(this.currentDir);
  }

  private async deleteItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    await webApi.sftpDelete(sid, entry.path);
    await this.load(this.currentDir);
  }

  private async uploadFiles(files: FileList | null): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid || !files?.length) return;
    const base = this.currentDir === '/' ? '' : this.currentDir;
    for (const file of Array.from(files)) {
      await webApi.sftpUploadFile(sid, `${base}/${file.name}`, file);
    }
    this.deps.toast(`已上传 ${files.length} 个文件`);
  }

  private renderTransfers(): void {
    const list = Array.from(this.transfers.values()).slice(-4).reverse();
    this.txSumEl.textContent = list.some((t) => t.status === 'running') ? '正在传输文件…' : `最近传输 (${list.length})`;
    this.txListEl.textContent = '';
    for (const t of list) {
      const pill = document.createElement('span');
      pill.className = 'tx-pill mono';
      const pct = t.total > 0 ? Math.min(100, Math.round((t.transferred / t.total) * 100)) : 100;
      pill.textContent = `${t.direction === 'upload' ? '↑' : '↓'} ${t.filename} · ${t.status === 'done' ? '完成' : t.status === 'running' ? `${pct}%` : t.status}`;
      if (t.status === 'running') {
        const cancel = document.createElement('button');
        cancel.type = 'button';
        cancel.textContent = '×';
        cancel.style.padding = '0 4px';
        cancel.addEventListener('click', () => void webApi.cancelTransfer(t.id));
        pill.appendChild(cancel);
      }
      this.txListEl.appendChild(pill);
    }
  }

  private bindEvents(): void {
    document.getElementById('sftp-up')?.addEventListener('click', () => {
      const parts = this.currentDir.split('/').filter(Boolean);
      parts.pop();
      void this.load(`/${parts.join('/')}`);
    });
    document.getElementById('sftp-home')?.addEventListener('click', () => void this.load('/'));
    document.getElementById('sftp-go')?.addEventListener('click', () => void this.load(this.pathInput.value));
    document.getElementById('sftp-refresh')?.addEventListener('click', () => void this.load(this.currentDir));
    document.getElementById('sftp-mkdir')?.addEventListener('click', async () => {
      const sid = this.deps.getActiveSshId();
      const name = await this.deps.promptText('新建远程目录名');
      if (sid && name) {
        await webApi.sftpMkdir(sid, `${this.currentDir === '/' ? '' : this.currentDir}/${name}`);
        await this.load(this.currentDir);
      }
    });
    const fileInput = document.getElementById('sftp-file-input') as HTMLInputElement;
    document.getElementById('sftp-upload')?.addEventListener('click', () => fileInput.click());
    fileInput.addEventListener('change', () => {
      void this.uploadFiles(fileInput.files);
      fileInput.value = '';
    });
    this.wrapEl.addEventListener('dragover', (e) => {
      e.preventDefault();
      this.wrapEl.classList.add('dragover');
    });
    this.wrapEl.addEventListener('dragleave', () => this.wrapEl.classList.remove('dragover'));
    this.wrapEl.addEventListener('drop', (e) => {
      e.preventDefault();
      this.wrapEl.classList.remove('dragover');
      void this.uploadFiles(e.dataTransfer?.files ?? null);
    });
    document.getElementById('cancel-editor')?.addEventListener('click', () => this.editorDlg.close());
    document.getElementById('editor-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      const sid = this.deps.getActiveSshId();
      if (!sid || !this.editingPath) return;
      await webApi.sftpWriteText(sid, this.editingPath, this.editorContent.value);
      this.editorDlg.close();
      this.deps.toast('已写回远端文件');
      await this.load(this.currentDir);
    });
  }
}
