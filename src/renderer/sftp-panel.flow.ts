import assert from 'node:assert/strict';
import { webApi } from './api-client.js';
import { SftpController } from './sftp-panel.js';
import type { SftpEntry } from '../shared/types.js';

class ClassList {
  private readonly names = new Set<string>();

  add(...items: string[]): void {
    for (const item of items) this.names.add(item);
  }

  remove(...items: string[]): void {
    for (const item of items) this.names.delete(item);
  }

  contains(item: string): boolean {
    return this.names.has(item);
  }

  toggle(item: string): boolean {
    if (this.names.has(item)) {
      this.names.delete(item);
      return false;
    }
    this.names.add(item);
    return true;
  }
}

class FakeEl {
  id = '';
  className = '';
  value = '';
  title = '';
  disabled = false;
  type = '';
  files: FileList | null = null;
  selectionStart = 0;
  selectionEnd = 0;
  shown = false;
  parent: FakeEl | null = null;
  children: FakeEl[] = [];
  classList = new ClassList();
  style: Record<string, string> = {};
  tag: string;
  text = '';
  listeners = new Map<string, Array<(event: FakeEvent) => void>>();

  constructor(tag: string) {
    this.tag = tag;
  }

  get textContent(): string {
    return this.text;
  }

  set textContent(value: string) {
    this.text = value;
    this.children = [];
  }

  appendChild(child: FakeEl): FakeEl {
    child.parent = this;
    this.children.push(child);
    return child;
  }

  addEventListener(type: string, fn: (event: FakeEvent) => void): void {
    const list = this.listeners.get(type) ?? [];
    list.push(fn);
    this.listeners.set(type, list);
  }

  dispatch(type: string): void {
    const event: FakeEvent = {
      stopPropagation() {},
      preventDefault() {},
    };
    let node: FakeEl | null = this;
    while (node) {
      for (const fn of node.listeners.get(type) ?? []) fn(event);
      node = node.parent;
    }
  }

  focus(): void {}
  select(): void {}
  click(): void {}
  showModal(): void {
    this.shown = true;
  }
  close(): void {
    this.shown = false;
  }
}

interface FakeEvent {
  stopPropagation(): void;
  preventDefault(): void;
}

function installDocument(): Map<string, FakeEl> {
  const byId = new Map<string, FakeEl>();
  const ids = [
    'sftp-path',
    'sftp-breadcrumbs',
    'sftp-edit-path',
    'sftp-go',
    'sftp-archive-current',
    'sftp-drag-overlay',
    'sftp-drag-target',
    'sftp-body',
    'sftp-table',
    'sftp-empty',
    'sftp-table-wrap',
    'transfer-list',
    'transfer-summary',
    'editor-dialog',
    'editor-content',
    'editor-path',
    'editor-gutter',
    'editor-dirty-badge',
    'editor-file-icon',
    'editor-cursor-pos',
    'editor-line-count',
    'editor-char-count',
    'editor-maximize-btn',
    'editor-reload-btn',
    'close-editor-btn',
    'sftp-up',
    'sftp-refresh',
    'sftp-mkdir',
    'sftp-upload',
    'sftp-file-input',
    'cancel-editor',
    'editor-form',
    'save-editor-btn',
  ];
  for (const id of ids) {
    const el = new FakeEl(id === 'editor-content' || id === 'sftp-path' ? 'input' : 'div');
    el.id = id;
    byId.set(id, el);
  }
  const documentShim = {
    getElementById(id: string) {
      return byId.get(id) ?? null;
    },
    createElement(tag: string) {
      return new FakeEl(tag);
    },
  };
  Object.assign(globalThis, { document: documentShim, requestAnimationFrame: (fn: () => void) => fn() });
  return byId;
}

function entry(name: string, kind: SftpEntry['kind']): SftpEntry {
  return {
    name,
    path: `/${name}`,
    kind,
    size: kind === 'dir' ? 0 : 4,
    mtimeMs: 1,
    mode: 0o644,
    permissions: kind === 'symlink' ? 'lrwxrwxrwx' : kind === 'dir' ? 'drwxr-xr-x' : '-rw-r--r--',
  };
}

function namesIn(body: FakeEl): FakeEl[] {
  const found: FakeEl[] = [];
  const walk = (node: FakeEl) => {
    if (node.className.includes('sftp-item-name')) found.push(node);
    for (const child of node.children) walk(child);
  };
  walk(body);
  return found;
}

async function waitFor(label: string, pred: () => boolean): Promise<void> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > 800) assert.fail(label);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/** 真实控制器上的点击和双击。列表失败是异步的，当前目录列表要留着。 */
export async function runSymlinkOpenFlow(): Promise<void> {
  const byId = installDocument();
  const lists: string[] = [];
  const reads: string[] = [];
  const toasts: string[] = [];
  const parent = [
    entry('logs', 'dir'),
    entry('plain.txt', 'file'),
    entry('file-link', 'symlink'),
    entry('note-link', 'symlink'),
    entry('keep-link', 'symlink'),
    entry('dangling', 'symlink'),
  ];
  const originalList = webApi.sftpList;
  const originalRead = webApi.sftpReadText;
  webApi.sftpList = async (_sid, path) => {
    lists.push(path);
    await new Promise((resolve) => setTimeout(resolve, 20));
    if (path === '/') return parent;
    if (path === '/logs' || path === '/keep-link') return [entry('inside.txt', 'file')];
    throw new Error(`列目录失败 ${path}`);
  };
  webApi.sftpReadText = async (_sid, path) => {
    reads.push(path);
    await new Promise((resolve) => setTimeout(resolve, 10));
    if (path === '/dangling') throw new Error('读取文件失败');
    return { content: `内容:${path}`, truncated: false };
  };

  try {
    const ctrl = new SftpController({
      getActiveSshId: () => 'ssh-1',
      toast: (msg) => toasts.push(msg),
      promptText: async () => null,
      confirm: async () => false,
    });
    const shown = await ctrl.load('/');
    assert.equal(shown, 'shown');
    assert.equal(ctrl.currentPath(), '/');
    const body = byId.get('sftp-body') as FakeEl;
    const dialog = byId.get('editor-dialog') as FakeEl;
    const editor = byId.get('editor-content') as FakeEl;

    const fileLink = namesIn(body).find((node) => node.textContent.includes('file-link'));
    assert.ok(fileLink);
    fileLink.dispatch('click');
    fileLink.dispatch('click');
    fileLink.dispatch('dblclick');
    await waitFor('单击和双击都该打开 file-link', () => editor.value === '内容:/file-link' && dialog.shown);
    assert.deepEqual(reads.filter((path) => path === '/file-link'), ['/file-link']);
    assert.equal(editor.value, '内容:/file-link');
    assert.equal(ctrl.currentPath(), '/');
    assert.equal(toasts.length, 0);
    assert.ok(namesIn(body).some((node) => node.textContent.includes('logs')));

    dialog.shown = false;
    const noteLink = namesIn(body).find((node) => node.textContent.includes('note-link'));
    assert.ok(noteLink);
    noteLink.dispatch('dblclick');
    await waitFor('只双击 note-link 也该打开编辑器', () => editor.value === '内容:/note-link' && dialog.shown);
    assert.equal(editor.value, '内容:/note-link');
    assert.equal(ctrl.currentPath(), '/');

    const beforeDirLists = lists.length;
    const beforeReads = reads.length;
    const keepLink = namesIn(body).find((node) => node.textContent.includes('keep-link'));
    assert.ok(keepLink);
    keepLink.dispatch('dblclick');
    await waitFor('指向目录的链接该进入目录', () => ctrl.currentPath() === '/keep-link');
    assert.ok(lists.slice(beforeDirLists).includes('/keep-link'));
    assert.equal(reads.length, beforeReads);
    assert.ok(namesIn(body).some((node) => node.textContent.includes('inside.txt')));

    await ctrl.load('/');
    const plain = namesIn(body).find((node) => node.textContent.includes('plain.txt'));
    assert.ok(plain);
    const listsBeforeFile = lists.length;
    plain.dispatch('click');
    await waitFor('普通文件该直接打开', () => editor.value === '内容:/plain.txt');
    assert.equal(lists.length, listsBeforeFile);
    assert.equal(editor.value, '内容:/plain.txt');

    const logs = namesIn(body).find((node) => node.textContent.includes('logs'));
    assert.ok(logs);
    logs.dispatch('dblclick');
    await waitFor('真实目录该列出来', () => ctrl.currentPath() === '/logs');
    assert.equal(reads.filter((path) => path === '/logs').length, 0);

    dialog.shown = false;
    const jumped = await ctrl.load('/missing');
    assert.equal(jumped, 'failed');
    assert.equal(ctrl.currentPath(), '/logs');
    assert.deepEqual(toasts, ['列目录失败 /missing']);
    assert.equal(dialog.shown, false);
    assert.equal(reads.includes('/missing'), false);

    toasts.length = 0;
    await ctrl.load('/');
    dialog.shown = false;
    const dangling = namesIn(body).find((node) => node.textContent.includes('dangling'));
    assert.ok(dangling);
    dangling.dispatch('click');
    await waitFor('悬空链接该尝试打开文件并提示读失败', () => toasts.includes('读取文件失败'));
    assert.equal(dialog.shown, false);
    assert.equal(ctrl.currentPath(), '/');
    assert.deepEqual(toasts, ['读取文件失败']);
    assert.ok(namesIn(body).some((node) => node.textContent.includes('file-link')));
  } finally {
    webApi.sftpList = originalList;
    webApi.sftpReadText = originalRead;
  }
}
