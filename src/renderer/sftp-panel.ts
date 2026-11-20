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

function getFileMeta(name: string, isDir: boolean): { icon: string; cls: string } {
  if (isDir) return { icon: '📁', cls: 'file-dir' };
  const dot = name.lastIndexOf('.');
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
  switch (ext) {
    case 'sh': case 'bash': case 'zsh': case 'bat': case 'cmd': case 'ps1':
      return { icon: '⚡', cls: 'file-sh' };
    case 'conf': case 'config': case 'yaml': case 'yml': case 'json': case 'toml': case 'ini': case 'env': case 'xml':
      return { icon: '⚙️', cls: 'file-conf' };
    case 'tar': case 'gz': case 'zip': case 'tgz': case '7z': case 'rar': case 'xz': case 'bz2':
      return { icon: '📦', cls: 'file-archive' };
    case 'log': case 'out': case 'err':
      return { icon: '📝', cls: 'file-log' };
    case 'key': case 'pem': case 'pub': case 'crt': case 'cer': case 'p12':
      return { icon: '🔑', cls: 'file-key' };
    case 'py': case 'rs': case 'js': case 'ts': case 'go': case 'java': case 'c': case 'cpp': case 'h': case 'html': case 'css':
      return { icon: '💻', cls: 'file-code' };
    default:
      return { icon: '📄', cls: 'file-default' };
  }
}

export class SftpController {
  private currentDir = '/';
  private showHidden = false;
  private lastSig = '';
  private readonly transfers = new Map<string, TransferTask>();
  private editingPath: string | null = null;

  private readonly pathInput = document.getElementById('sftp-path') as HTMLInputElement;
  private readonly breadcrumbsEl = document.getElementById('sftp-breadcrumbs') as HTMLDivElement;
  private readonly editPathBtn = document.getElementById('sftp-edit-path') as HTMLButtonElement;
  private readonly goBtn = document.getElementById('sftp-go') as HTMLButtonElement;
  private readonly archiveCurrentBtn = document.getElementById('sftp-archive-current') as HTMLButtonElement | null;
  private readonly dragOverlay = document.getElementById('sftp-drag-overlay') as HTMLDivElement | null;
  private readonly dragTarget = document.getElementById('sftp-drag-target') as HTMLDivElement | null;
  private readonly bodyEl = document.getElementById('sftp-body') as HTMLTableSectionElement;
  private readonly tableEl = document.getElementById('sftp-table') as HTMLTableElement;
  private readonly emptyEl = document.getElementById('sftp-empty') as HTMLDivElement;
  private readonly wrapEl = document.getElementById('sftp-table-wrap') as HTMLDivElement;
  private readonly txListEl = document.getElementById('transfer-list') as HTMLDivElement;
  private readonly txSumEl = document.getElementById('transfer-summary') as HTMLSpanElement;
  private readonly editorDlg = document.getElementById('editor-dialog') as HTMLDialogElement;
  private readonly editorContent = document.getElementById('editor-content') as HTMLTextAreaElement;
  private readonly editorPathEl = document.getElementById('editor-path') as HTMLParagraphElement;
  private readonly editorGutter = document.getElementById('editor-gutter') as HTMLDivElement | null;
  private readonly editorDirtyBadge = document.getElementById('editor-dirty-badge') as HTMLSpanElement | null;
  private readonly editorFileIcon = document.getElementById('editor-file-icon') as HTMLSpanElement | null;
  private readonly editorCursorPos = document.getElementById('editor-cursor-pos') as HTMLSpanElement | null;
  private readonly editorLineCount = document.getElementById('editor-line-count') as HTMLSpanElement | null;
  private readonly editorCharCount = document.getElementById('editor-char-count') as HTMLSpanElement | null;
  private readonly editorMaximizeBtn = document.getElementById('editor-maximize-btn') as HTMLButtonElement | null;
  private readonly editorReloadBtn = document.getElementById('editor-reload-btn') as HTMLButtonElement | null;
  private readonly closeEditorBtn = document.getElementById('close-editor-btn') as HTMLButtonElement | null;
  private editorOriginalContent = '';
  private isEditorDirty = false;

  constructor(private readonly deps: SftpControllerDeps) {
    this.bindEvents();
  }

  setShowHidden(flag: boolean): void {
    this.showHidden = flag;
    if (this.deps.getActiveSshId()) void this.load(this.currentDir);
  }

  syncSession(): void {
    this.lastSig = '';
    const active = Boolean(this.deps.getActiveSshId());
    for (const id of ['sftp-up', 'sftp-path', 'sftp-edit-path', 'sftp-go', 'sftp-mkdir', 'sftp-upload', 'sftp-archive-current', 'sftp-refresh']) {
      const el = document.getElementById(id) as HTMLButtonElement | HTMLInputElement | null;
      if (el) el.disabled = !active;
    }
    if (active) {
      void this.load(this.currentDir, true);
    } else {
      this.lastSig = '';
      this.tableEl.classList.add('hidden');
      this.emptyEl.classList.remove('hidden');
      this.emptyEl.textContent = '空闲。连上 SSH 会话后，这里显示该会话的当前目录。';
      this.bodyEl.textContent = '';
      if (this.breadcrumbsEl) this.breadcrumbsEl.textContent = '';
    }
  }

  private renderBreadcrumbs(dirPath: string): void {
    if (!this.breadcrumbsEl) return;
    this.breadcrumbsEl.textContent = '';
    const norm = dirPath === '/' ? '/' : dirPath.replace(/\/+$/, '');
    const parts = norm.split('/').filter(Boolean);

    const rootChip = document.createElement('span');
    rootChip.className = `breadcrumb-chip ${parts.length === 0 ? 'active' : ''}`;
    rootChip.textContent = '🏠 /';
    rootChip.title = '根目录 (/)';
    rootChip.addEventListener('click', (e) => {
      e.stopPropagation();
      void this.load('/');
    });
    this.breadcrumbsEl.appendChild(rootChip);

    let accum = '';
    for (let i = 0; i < parts.length; i++) {
      const seg = parts[i];
      accum += `/${seg}`;
      const targetPath = accum;
      const isLast = i === parts.length - 1;

      const sep = document.createElement('span');
      sep.className = 'breadcrumb-sep';
      sep.textContent = '›';
      this.breadcrumbsEl.appendChild(sep);

      const chip = document.createElement('span');
      chip.className = `breadcrumb-chip ${isLast ? 'active' : ''}`;
      chip.textContent = seg;
      chip.title = targetPath;
      chip.addEventListener('click', (e) => {
        e.stopPropagation();
        void this.load(targetPath);
      });
      this.breadcrumbsEl.appendChild(chip);
    }
  }

  private showPathInput(show: boolean): void {
    if (!this.breadcrumbsEl || !this.pathInput) return;
    if (show) {
      this.breadcrumbsEl.classList.add('hidden');
      this.editPathBtn?.classList.add('hidden');
      this.pathInput.classList.remove('hidden');
      this.goBtn?.classList.remove('hidden');
      this.pathInput.focus();
      this.pathInput.select();
    } else {
      this.breadcrumbsEl.classList.remove('hidden');
      this.editPathBtn?.classList.remove('hidden');
      this.pathInput.classList.add('hidden');
      this.goBtn?.classList.add('hidden');
    }
  }

  async load(dirPath: string, quiet = false): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    this.tableEl.classList.add('loading');
    try {
      const list = await webApi.sftpList(sid, dirPath);
      this.currentDir = dirPath || '/';
      this.pathInput.value = this.currentDir;
      this.renderBreadcrumbs(this.currentDir);
      this.showPathInput(false);
      const visible = list.filter((e) => this.showHidden || !e.name.startsWith('.'));
      const sig = `${this.currentDir}|${visible.map((e) => `${e.path}:${e.size}:${e.mtimeMs}:${e.mode}`).join('|')}`;
      if (sig === this.lastSig) return;
      this.lastSig = sig;
      this.renderEntries(visible);
    } catch (err) {
      if (!quiet) this.deps.toast(err instanceof Error ? err.message : 'SFTP 读取失败');
    } finally {
      this.tableEl.classList.remove('loading');
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
      const isDir = entry.kind === 'dir';
      const meta = getFileMeta(entry.name, isDir);
      nameBtn.textContent = `${meta.icon} ${entry.name}`;
      nameBtn.style.cursor = 'pointer';
      nameBtn.className = `sftp-item-name ${meta.cls}`;
      nameTd.appendChild(nameBtn);
      tr.appendChild(nameTd);

      for (const val of [entry.permissions, isDir ? '--' : formatBytes(entry.size), entry.mtimeMs ? new Date(entry.mtimeMs).toLocaleString() : '--']) {
        const td = document.createElement('td');
        td.className = 'mono';
        td.textContent = val;
        tr.appendChild(td);
      }

      const opsTd = document.createElement('td');
      opsTd.style.display = 'flex';
      opsTd.style.gap = '4px';
      if (!isDir) {
        this.addOpBtn(opsTd, '编辑', () => void this.openEditor(entry.path));
        this.addOpBtn(opsTd, '下载', () => this.triggerDownload(entry.path));
      } else {
        this.addOpBtn(opsTd, '打包', () => this.triggerArchiveDownload(entry.path));
      }
      this.addOpBtn(opsTd, '改名', () => void this.renameItem(entry));
      this.addOpBtn(opsTd, '权限', () => void this.chmodItem(entry));
      this.addOpBtn(opsTd, '删', () => void this.deleteItem(entry));
      tr.appendChild(opsTd);

      const handleOpen = () => {
        if (isDir) {
          void this.load(entry.path);
        } else if (entry.kind === 'file') {
          void this.openEditor(entry.path);
        } else {
          void this.load(entry.path).catch(() => void this.openEditor(entry.path));
        }
      };

      nameBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        handleOpen();
      });
      tr.addEventListener('dblclick', (e) => {
        e.stopPropagation();
        handleOpen();
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
      const fileName = remotePath.split('/').pop() || '文件';
      const meta = getFileMeta(fileName, false);
      if (this.editorFileIcon) this.editorFileIcon.textContent = meta.icon;
      this.editorPathEl.textContent = res.truncated ? `${remotePath}（仅预览前 256KB）` : remotePath;
      this.editorPathEl.title = remotePath;
      this.editorContent.value = res.content;
      this.editorOriginalContent = res.content;
      this.checkEditorDirty();
      this.updateEditorGutter();
      this.updateEditorCursorInfo();
      this.editorDlg.showModal();
      requestAnimationFrame(() => this.editorContent.focus());
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '读取文件失败');
    }
  }

  private updateEditorGutter(): void {
    if (!this.editorGutter) return;
    const lines = this.editorContent.value.split('\n');
    const count = lines.length;
    let nums = '';
    for (let i = 1; i <= count; i++) {
      nums += (i === 1 ? '' : '\n') + i;
    }
    this.editorGutter.textContent = nums;
    if (this.editorLineCount) this.editorLineCount.textContent = `共 ${count} 行`;
    if (this.editorCharCount) this.editorCharCount.textContent = `${this.editorContent.value.length} 字符`;
  }

  private updateEditorCursorInfo(): void {
    if (!this.editorCursorPos) return;
    const pos = this.editorContent.selectionStart ?? 0;
    const textBefore = this.editorContent.value.substring(0, pos);
    const lines = textBefore.split('\n');
    const lineNum = lines.length;
    const colNum = lines[lines.length - 1].length + 1;
    this.editorCursorPos.textContent = `行 ${lineNum}, 列 ${colNum}`;
  }

  private checkEditorDirty(): void {
    if (!this.editorDirtyBadge) return;
    const dirty = this.editorContent.value !== this.editorOriginalContent;
    this.isEditorDirty = dirty;
    this.editorDirtyBadge.className = `editor-dirty-badge ${dirty ? 'dirty' : 'clean'}`;
    this.editorDirtyBadge.textContent = dirty ? '● 未保存' : '✓ 已保存';
  }

  private async saveEditorContent(): Promise<boolean> {
    const sid = this.deps.getActiveSshId();
    if (!sid || !this.editingPath) return false;
    try {
      await webApi.sftpWriteText(sid, this.editingPath, this.editorContent.value);
      this.editorOriginalContent = this.editorContent.value;
      this.checkEditorDirty();
      this.deps.toast(`已写回远端文件: ${this.editingPath.split('/').pop()}`);
      void this.load(this.currentDir, true);
      return true;
    } catch (err) {
      this.deps.toast(err instanceof Error ? `保存失败: ${err.message}` : '保存失败');
      return false;
    }
  }

  private handleCloseEditor(): void {
    if (this.isEditorDirty) {
      if (!window.confirm('当前文件存在未保存的修改，确定放弃修改并关闭吗？')) {
        return;
      }
    }
    this.editorDlg.close();
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

  private triggerArchiveDownload(remotePath: string): void {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const a = document.createElement('a');
    a.href = webApi.sftpArchiveDownloadUrl(sid, remotePath);
    const name = remotePath === '/' ? 'root' : (remotePath.split('/').filter(Boolean).pop() || 'archive');
    a.download = `${name}.tar.gz`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    this.deps.toast(`开始流式打包下载 ${name}.tar.gz`);
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
    this.editPathBtn?.addEventListener('click', () => this.showPathInput(true));
    this.breadcrumbsEl?.addEventListener('dblclick', () => this.showPathInput(true));
    this.pathInput?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        void this.load(this.pathInput.value);
        this.showPathInput(false);
      } else if (e.key === 'Escape') {
        this.showPathInput(false);
      }
    });
    this.goBtn?.addEventListener('click', () => {
      void this.load(this.pathInput.value);
      this.showPathInput(false);
    });
    document.getElementById('sftp-refresh')?.addEventListener('click', () => void this.load(this.currentDir));
    document.getElementById('sftp-mkdir')?.addEventListener('click', async () => {
      const sid = this.deps.getActiveSshId();
      const name = await this.deps.promptText('新建远程目录名');
      if (sid && name) {
        await webApi.sftpMkdir(sid, `${this.currentDir === '/' ? '' : this.currentDir}/${name}`);
        await this.load(this.currentDir);
      }
    });
    this.archiveCurrentBtn?.addEventListener('click', () => this.triggerArchiveDownload(this.currentDir));
    const fileInput = document.getElementById('sftp-file-input') as HTMLInputElement;
    document.getElementById('sftp-upload')?.addEventListener('click', () => fileInput.click());
    fileInput.addEventListener('change', () => {
      void this.uploadFiles(fileInput.files);
      fileInput.value = '';
    });
    let dragCounter = 0;
    this.wrapEl.addEventListener('dragenter', (e) => {
      e.preventDefault();
      dragCounter++;
      if (this.dragOverlay) {
        if (this.dragTarget) this.dragTarget.textContent = this.currentDir;
        this.dragOverlay.classList.remove('hidden');
      }
    });
    this.wrapEl.addEventListener('dragover', (e) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
      if (this.dragOverlay && this.dragOverlay.classList.contains('hidden')) {
        if (this.dragTarget) this.dragTarget.textContent = this.currentDir;
        this.dragOverlay.classList.remove('hidden');
      }
    });
    this.wrapEl.addEventListener('dragleave', (e) => {
      e.preventDefault();
      dragCounter--;
      if (dragCounter <= 0) {
        dragCounter = 0;
        this.dragOverlay?.classList.add('hidden');
      }
    });
    this.wrapEl.addEventListener('drop', (e) => {
      e.preventDefault();
      dragCounter = 0;
      this.dragOverlay?.classList.add('hidden');
      void this.uploadFiles(e.dataTransfer?.files ?? null);
    });
    this.editorContent.addEventListener('input', () => {
      this.updateEditorGutter();
      this.checkEditorDirty();
      this.updateEditorCursorInfo();
    });
    this.editorContent.addEventListener('scroll', () => {
      if (this.editorGutter) this.editorGutter.scrollTop = this.editorContent.scrollTop;
    });
    this.editorContent.addEventListener('click', () => this.updateEditorCursorInfo());
    this.editorContent.addEventListener('keyup', () => this.updateEditorCursorInfo());
    this.editorContent.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void this.saveEditorContent();
      } else if (e.key === 'Tab') {
        e.preventDefault();
        const start = this.editorContent.selectionStart;
        const end = this.editorContent.selectionEnd;
        const val = this.editorContent.value;
        if (start === end) {
          this.editorContent.value = val.substring(0, start) + '  ' + val.substring(end);
          this.editorContent.selectionStart = this.editorContent.selectionEnd = start + 2;
        } else {
          const before = val.substring(0, start);
          const after = val.substring(end);
          const lineStart = before.lastIndexOf('\n') + 1;
          const fullBlock = val.substring(lineStart, end);
          const lines = fullBlock.split('\n');
          if (e.shiftKey) {
            const outdented = lines.map((l) => (l.startsWith('  ') ? l.slice(2) : l.startsWith(' ') ? l.slice(1) : l)).join('\n');
            this.editorContent.value = val.substring(0, lineStart) + outdented + after;
            this.editorContent.selectionStart = start;
            this.editorContent.selectionEnd = lineStart + outdented.length;
          } else {
            const indented = lines.map((l) => '  ' + l).join('\n');
            this.editorContent.value = val.substring(0, lineStart) + indented + after;
            this.editorContent.selectionStart = start + 2;
            this.editorContent.selectionEnd = lineStart + indented.length;
          }
        }
        this.updateEditorGutter();
        this.checkEditorDirty();
        this.updateEditorCursorInfo();
      } else if (e.key === 'Escape') {
        e.preventDefault();
        this.handleCloseEditor();
      }
    });

    this.editorMaximizeBtn?.addEventListener('click', () => {
      const isMax = this.editorDlg.classList.toggle('maximized');
      if (this.editorMaximizeBtn) {
        this.editorMaximizeBtn.textContent = isMax ? '❐ 还原' : '⛶ 最大化';
      }
    });

    this.editorReloadBtn?.addEventListener('click', () => {
      if (!this.editingPath) return;
      if (this.isEditorDirty && !window.confirm('重新载入将覆盖未保存的修改，确定继续吗？')) {
        return;
      }
      void this.openEditor(this.editingPath);
    });

    document.getElementById('cancel-editor')?.addEventListener('click', () => this.handleCloseEditor());
    this.closeEditorBtn?.addEventListener('click', () => this.handleCloseEditor());
    document.getElementById('editor-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      const ok = await this.saveEditorContent();
      if (ok) this.editorDlg.close();
    });
  }
}
