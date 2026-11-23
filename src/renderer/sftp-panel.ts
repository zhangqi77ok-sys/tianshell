import type { SftpEntry, TransferTask } from '../shared/types.js';
import { webApi } from './api-client.js';
import { editorWriteGuard } from './session-logic.js';
import { chmodPrompt, deletePrompt, editorSaveBlock, failedLoadEffect, keepEditorOpen, parseOctalMode, planUploads } from './sftp-logic.js';

export interface SftpControllerDeps {
  getActiveSshId: () => string | null;
  toast: (msg: string, kind?: 'info' | 'error') => void;
  promptText: (title: string, defaultVal?: string) => Promise<string | null>;
  confirm: (title: string, body: string, okLabel?: string) => Promise<boolean>;
  onDirChange?: (dir: string) => void;
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

function getFileMeta(name: string, kind: 'dir' | 'file' | 'symlink'): { icon: string; cls: string } {
  if (kind === 'symlink') return { icon: '🔗', cls: 'file-link' };
  if (kind === 'dir') return { icon: '📁', cls: 'file-dir' };
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
  private boundSession: string | null = null;
  private showHidden = false;
  private lastSig = '';
  private loadTicket = 0;
  private readonly transfers = new Map<string, TransferTask>();
  private editingPath: string | null = null;
  private editingSession: string | null = null;
  private editorSaving = false;
  private editorTruncated = false;
  private editorClosePending = false;
  private listedNames: string[] = [];
  private haveListing = false;
  private readonly dirCache = new Map<string, { list: SftpEntry[]; timestamp: number }>();
  private static readonly CACHE_TTL_MS = 30_000;
  private static readonly MAX_CACHE_ENTRIES = 100;

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
  private readonly deps: SftpControllerDeps;

  constructor(deps: SftpControllerDeps) {
    this.deps = deps;
    this.bindEvents();
  }

  setShowHidden(flag: boolean): void {
    this.showHidden = flag;
    if (this.deps.getActiveSshId()) void this.load(this.currentDir);
  }

  currentPath(): string {
    return this.currentDir;
  }

  invalidateCache(dirPath?: string): void {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    if (dirPath) {
      this.dirCache.delete(this.getCacheKey(sid, dirPath));
    } else {
      for (const key of this.dirCache.keys()) {
        if (key.startsWith(`${sid}:`)) this.dirCache.delete(key);
      }
    }
  }

  private getCacheKey(sid: string, dirPath: string): string {
    const norm = dirPath === '/' ? '/' : dirPath.replace(/\/+$/, '');
    return `${sid}:${norm}`;
  }

  syncSession(): void {
    const sid = this.deps.getActiveSshId();
    const switched = sid !== this.boundSession;
    this.boundSession = sid;
    const active = Boolean(sid);
    for (const id of ['sftp-up', 'sftp-path', 'sftp-edit-path', 'sftp-go', 'sftp-mkdir', 'sftp-upload', 'sftp-archive-current', 'sftp-refresh']) {
      const el = document.getElementById(id) as HTMLButtonElement | HTMLInputElement | null;
      if (el) el.disabled = !active;
    }
    if (!sid) {
      this.currentDir = '/';
      this.listedNames = [];
      this.haveListing = false;
      this.showListingMessage('空闲。连上 SSH 会话后，这里显示该会话的当前目录。', true);
      if (this.breadcrumbsEl) this.breadcrumbsEl.textContent = '';
      this.deps.onDirChange?.('/');
      return;
    }
    if (switched) {
      this.currentDir = '/';
      this.listedNames = [];
      this.haveListing = false;
      this.showListingMessage('正在读取 / …', true);
      this.deps.onDirChange?.('/');
    }
    void this.load(this.currentDir);
  }

  private showListingMessage(message: string, clearSig: boolean): void {
    if (clearSig) this.lastSig = '';
    this.bodyEl.textContent = '';
    this.tableEl.classList.add('hidden');
    this.emptyEl.classList.remove('hidden');
    this.emptyEl.textContent = message;
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

  /**
   * `notify`：跳转失败时提示，并留下已经列出的目录。
   * `defer`：这次失败不提示、不换列表，把结果交回调用方。
   * 符号链接需要后者。`load` 自己接住列表错误后会正常返回，调用方的 catch 接不到。
   */
  async load(dirPath: string, failure: 'notify' | 'defer' = 'notify', force = false): Promise<'shown' | 'failed' | 'idle'> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return 'idle';
    const requested = dirPath || '/';
    const cacheKey = this.getCacheKey(sid, requested);
    const now = Date.now();
    const cached = !force ? this.dirCache.get(cacheKey) : undefined;

    if (cached && (now - cached.timestamp < SftpController.CACHE_TTL_MS)) {
      this.currentDir = requested;
      this.pathInput.value = this.currentDir;
      this.renderBreadcrumbs(this.currentDir);
      this.haveListing = true;
      this.showPathInput(false);
      this.deps.onDirChange?.(this.currentDir);
      this.listedNames = cached.list.map((entry) => entry.name);
      const visible = cached.list.filter((e) => this.showHidden || !e.name.startsWith('.'));
      const sig = `${sid}|${this.currentDir}|${visible.map((e) => `${e.path}:${e.size}:${e.mtimeMs}:${e.mode}`).join('|')}`;
      if (sig !== this.lastSig) {
        this.lastSig = sig;
        this.renderEntries(visible);
      }
      return 'shown';
    }

    const ticket = ++this.loadTicket;
    this.tableEl.classList.add('loading');
    try {
      const list = await webApi.sftpList(sid, requested);
      if (!this.listingCurrent(sid, ticket)) return 'idle';
      if (this.dirCache.size >= SftpController.MAX_CACHE_ENTRIES) {
        const oldest = this.dirCache.keys().next().value;
        if (oldest) this.dirCache.delete(oldest);
      }
      this.dirCache.set(cacheKey, { list, timestamp: Date.now() });
      this.currentDir = requested;
      this.pathInput.value = this.currentDir;
      this.renderBreadcrumbs(this.currentDir);
      this.haveListing = true;
      this.showPathInput(false);
      this.deps.onDirChange?.(this.currentDir);
      this.listedNames = list.map((entry) => entry.name);
      const visible = list.filter((e) => this.showHidden || !e.name.startsWith('.'));
      const sig = `${sid}|${this.currentDir}|${visible.map((e) => `${e.path}:${e.size}:${e.mtimeMs}:${e.mode}`).join('|')}`;
      if (sig === this.lastSig) return 'shown';
      this.lastSig = sig;
      this.renderEntries(visible);
      return 'shown';
    } catch (err) {
      if (!this.listingCurrent(sid, ticket)) return 'idle';
      if (failure === 'defer') return 'failed';
      const message = err instanceof Error ? err.message : 'SFTP 读取失败';
      if (failedLoadEffect(this.haveListing) === 'keep') {
        this.deps.toast(message, 'error');
        if (!this.pathInput.classList.contains('hidden')) this.pathInput.focus();
        return 'failed';
      }
      this.listedNames = [];
      this.showListingMessage(message, true);
      return 'failed';
    } finally {
      if (this.listingCurrent(sid, ticket)) this.tableEl.classList.remove('loading');
    }
  }

  /** 后发出的请求，以及仍是这个会话的请求，才允许改列表。 */
  private listingCurrent(sessionId: string, ticket: number): boolean {
    return this.deps.getActiveSshId() === sessionId && ticket === this.loadTicket;
  }

  onTransferProgress(task: TransferTask): void {
    this.transfers.set(task.id, task);
    this.renderTransfers();
    if (task.status === 'done' && task.direction === 'upload') {
      this.invalidateCache(this.currentDir);
      void this.load(this.currentDir, 'notify', true);
    }
  }

  private renderEntries(entries: SftpEntry[]): void {
    this.bodyEl.textContent = '';
    if (!entries.length) {
      this.tableEl.classList.add('hidden');
      this.emptyEl.classList.remove('hidden');
      this.emptyEl.textContent = '这个目录是空的。';
      return;
    }
    this.tableEl.classList.remove('hidden');
    this.emptyEl.classList.add('hidden');
    for (const entry of entries) {
      const tr = document.createElement('tr');
      const nameTd = document.createElement('td');
      const nameBtn = document.createElement('span');
      const isDir = entry.kind === 'dir';
      const meta = getFileMeta(entry.name, entry.kind);
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
          void this.openSymlink(entry.path);
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

  /** 先列目录。列开了就是指向目录的链接。列不开再按文件打开，当前列表留着。 */
  private async openSymlink(path: string): Promise<void> {
    const outcome = await this.load(path, 'defer');
    if (outcome === 'failed') await this.openEditor(path);
  }

  private async openEditor(remotePath: string): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) {
      this.deps.toast('没有仍连接的 SSH 会话，读不了这个文件', 'error');
      return;
    }
    try {
      const res = await webApi.sftpReadText(sid, remotePath);
      this.editingPath = remotePath;
      this.editingSession = sid;
      this.editorTruncated = res.truncated;
      this.setSaveEnabled(!res.truncated);
      const fileName = remotePath.split('/').pop() || '文件';
      const meta = getFileMeta(fileName, 'file');
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
      this.deps.toast(err instanceof Error ? err.message : '读取文件失败', 'error');
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

  private setSaveEnabled(enabled: boolean): void {
    const saveBtn = document.getElementById('save-editor-btn') as HTMLButtonElement | null;
    if (!saveBtn) return;
    saveBtn.disabled = !enabled;
    saveBtn.textContent = enabled ? '保存写回 (Ctrl+S)' : '不能写回';
  }

  private async saveEditorContent(): Promise<boolean> {
    const blocked = editorSaveBlock(this.editorTruncated);
    if (blocked) {
      this.deps.toast(blocked, 'error');
      return false;
    }
    const decision = editorWriteGuard(this.editingSession, this.deps.getActiveSshId(), this.editorSaving);
    if (decision === 'idle' || decision === 'busy' || !this.editingPath || !this.editingSession) return false;
    if (decision === 'disconnected') {
      this.deps.toast('这个会话已经断开，修改还留在编辑器里', 'error');
      return false;
    }
    const sid = this.editingSession;
    const path = this.editingPath;
    this.editorSaving = true;
    try {
      await webApi.sftpWriteText(sid, path, this.editorContent.value);
      this.editorOriginalContent = this.editorContent.value;
      this.checkEditorDirty();
      this.deps.toast(`已写回远端文件: ${path.split('/').pop()}`);
      this.invalidateCache(this.currentDir);
      void this.load(this.currentDir, 'notify', true);
      return true;
    } catch (err) {
      this.deps.toast(err instanceof Error ? `保存失败: ${err.message}` : '保存失败', 'error');
      return false;
    } finally {
      this.editorSaving = false;
    }
  }

  private finishEditor(): void {
    this.editingPath = null;
    this.editingSession = null;
    this.editorTruncated = false;
    this.isEditorDirty = false;
    this.setSaveEnabled(true);
    this.editorDlg.close();
  }

  private async handleCloseEditor(): Promise<void> {
    if (this.editorClosePending) return;
    if (this.isEditorDirty) {
      this.editorClosePending = true;
      let discard = false;
      try {
        discard = await this.deps.confirm('放弃未保存的修改？', '关闭后，这次编辑不会写回远端。', '放弃修改');
      } finally {
        this.editorClosePending = false;
      }
      if (keepEditorOpen(true, discard)) return;
    }
    this.finishEditor();
  }

  private async triggerDownload(remotePath: string): Promise<void> {
    const name = remotePath.split('/').pop() || 'file';
    await this.startGrantedDownload(remotePath, 'file', name);
  }

  private async triggerArchiveDownload(remotePath: string): Promise<void> {
    const name = remotePath === '/' ? 'root' : (remotePath.split('/').filter(Boolean).pop() || 'archive');
    const started = await this.startGrantedDownload(remotePath, 'archive', `${name}.tar.gz`);
    if (started) this.deps.toast(`开始流式打包下载 ${name}.tar.gz`);
  }

  private async startGrantedDownload(remotePath: string, kind: 'file' | 'archive', filename: string): Promise<boolean> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return false;
    try {
      const ticket = await webApi.sftpDownloadGrant(sid, remotePath, kind);
      const href = ticket.href;
      if (!href.startsWith('/api/downloads/') || href.includes('token=')) {
        this.deps.toast('下载地址无效', 'error');
        return false;
      }
      const a = document.createElement('a');
      a.href = href;
      a.rel = 'noreferrer';
      a.referrerPolicy = 'no-referrer';
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      a.remove();
      return true;
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '下载没有开始', 'error');
      return false;
    }
  }

  private async renameItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const nextName = await this.deps.promptText(`重命名 ${entry.name}`, entry.name);
    if (!nextName || nextName === entry.name) return;
    if (nextName.includes('/') || nextName.includes('\\')) {
      this.deps.toast('名字里不能带斜杠', 'error');
      return;
    }
    const slash = entry.path.lastIndexOf('/');
    const parent = slash <= 0 ? '' : entry.path.slice(0, slash);
    try {
      await webApi.sftpRename(sid, entry.path, `${parent}/${nextName}`);
      this.invalidateCache(parent || '/');
      await this.load(parent || '/', 'notify', true);
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '重命名失败', 'error');
    }
  }

  private async chmodItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    let initialMode = entry.mode;
    if (entry.kind === 'symlink') {
      try {
        const res = await webApi.sftpStat(sid, entry.path);
        initialMode = res.mode;
      } catch {
        // 若目标不存在（悬空链接）或读取失败，保留链接自身的 mode
      }
    }
    const raw = await this.deps.promptText(chmodPrompt(entry.kind), initialMode.toString(8));
    if (!raw) return;
    const mode = parseOctalMode(raw);
    if (mode === null) return this.deps.toast('权限格式无效，请用三位或四位八进制', 'error');
    try {
      await webApi.sftpChmod(sid, entry.path, mode);
      this.invalidateCache(this.currentDir);
      await this.load(this.currentDir, 'notify', true);
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '修改权限失败', 'error');
    }
  }

  private async deleteItem(entry: SftpEntry): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid) return;
    const prompt = deletePrompt(entry.kind, entry.name);
    const ok = await this.deps.confirm(prompt.title, prompt.body);
    if (!ok) return;
    try {
      await webApi.sftpDelete(sid, entry.path);
      this.invalidateCache(this.currentDir);
      await this.load(this.currentDir, 'notify', true);
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '删除失败', 'error');
    }
  }

  private async uploadFiles(files: FileList | null): Promise<void> {
    const sid = this.deps.getActiveSshId();
    if (!sid || !files?.length) return;
    const dir = this.currentDir;
    const batch = Array.from(files);
    const plan = planUploads(dir, this.listedNames, batch.map((file) => ({ name: file.name, size: file.size })));
    for (const item of plan.skipped) this.deps.toast(item.reason, 'error');
    let send = plan.send;
    if (plan.overwrite.length) {
      const preview = plan.overwrite.slice(0, 6).join('、');
      const more = plan.overwrite.length > 6 ? ` 等 ${plan.overwrite.length} 个` : '';
      const ok = await this.deps.confirm(
        `覆盖 ${plan.overwrite.length} 个同名文件？`,
        `这些文件会被替换：${preview}${more}`,
        '覆盖',
      );
      if (!ok) {
        const skip = new Set(plan.overwrite);
        send = send.filter((item) => !skip.has(item.name));
        if (!send.length) {
          this.deps.toast('没有上传，同名文件保持原样');
          return;
        }
      }
    }
    if (!send.length) return;
    const byName = new Map(batch.map((file) => [file.name, file]));
    let failed = 0;
    let firstError = '';
    let uploaded = 0;
    for (const item of send) {
      const file = byName.get(item.name);
      if (!file) continue;
      try {
        await webApi.sftpUploadFile(sid, item.path, file);
        uploaded += 1;
      } catch (err) {
        failed += 1;
        if (!firstError) firstError = err instanceof Error ? err.message : '上传失败';
      }
    }
    if (failed === 0) {
      this.deps.toast(`已上传 ${uploaded} 个文件`);
      return;
    }
    this.deps.toast(uploaded > 0 ? `${firstError}（${uploaded} 个已上传，${failed} 个失败）` : firstError, 'error');
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
        e.preventDefault();
        void this.load(this.pathInput.value);
      } else if (e.key === 'Escape') {
        this.pathInput.value = this.currentDir;
        this.showPathInput(false);
      }
    });
    this.goBtn?.addEventListener('click', () => {
      void this.load(this.pathInput.value);
    });
    document.getElementById('sftp-refresh')?.addEventListener('click', () => {
      this.invalidateCache(this.currentDir);
      void this.load(this.currentDir, 'notify', true);
    });
    document.getElementById('sftp-mkdir')?.addEventListener('click', async () => {
      const sid = this.deps.getActiveSshId();
      const dir = this.currentDir;
      const name = await this.deps.promptText('新建远程目录名');
      if (!sid || !name) return;
      if (name.includes('/') || name.includes('\\')) {
        this.deps.toast('目录名里不能带斜杠', 'error');
        return;
      }
      try {
        await webApi.sftpMkdir(sid, `${dir === '/' ? '' : dir}/${name}`);
        this.invalidateCache(dir);
        await this.load(dir, 'notify', true);
      } catch (err) {
        this.deps.toast(err instanceof Error ? err.message : '新建目录失败', 'error');
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
        void this.handleCloseEditor();
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
      const path = this.editingPath;
      void (async () => {
        if (this.isEditorDirty) {
          const ok = await this.deps.confirm('重新载入这个文件？', '未保存的修改会被远端内容覆盖。', '重新载入');
          if (!ok) return;
        }
        await this.openEditor(path);
      })();
    });

    this.editorDlg.addEventListener('cancel', (event) => {
      event.preventDefault();
      void this.handleCloseEditor();
    });
    document.getElementById('cancel-editor')?.addEventListener('click', () => void this.handleCloseEditor());
    this.closeEditorBtn?.addEventListener('click', () => void this.handleCloseEditor());
    document.getElementById('editor-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      const ok = await this.saveEditorContent();
      if (ok) this.finishEditor();
    });
  }
}
