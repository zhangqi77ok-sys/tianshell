/** 文件视图里和 DOM 无关的决定，方便锁住编辑、上传和权限。 */

export const MAX_UPLOAD_BYTES = 64 * 1024 * 1024;

export interface NamedSize {
  name: string;
  size: number;
}

export interface UploadItem {
  name: string;
  path: string;
}

export interface UploadSkip {
  name: string;
  reason: string;
}

export interface UploadPlan {
  send: UploadItem[];
  skipped: UploadSkip[];
  overwrite: string[];
}

/** 只打开了前面一段时，写回会用这段盖掉整个远端文件。 */
export function editorSaveBlock(truncated: boolean): string | null {
  return truncated ? '这个文件只打开了前面一段，写回会截掉后面的内容' : null;
}

/** 有未保存修改、又还没确认放弃时，编辑器必须留在打开状态。 */
export function keepEditorOpen(dirty: boolean, discardConfirmed: boolean): boolean {
  return dirty && !discardConfirmed;
}

/** 已经列出过目录时，跳转失败不能把这份列表和同名检查清掉。 */
export function failedLoadEffect(haveListing: boolean): 'keep' | 'show-error' {
  return haveListing ? 'keep' : 'show-error';
}

/** 把名字接到当前目录下。斜杠、空名和 .. 都不能出去。 */
export function remoteChildPath(dir: string, name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed || trimmed === '.' || trimmed === '..') return null;
  if (/[/\\\0]/.test(trimmed)) return null;
  const base = !dir || dir === '/' ? '' : dir.replace(/\/+$/, '');
  return `${base}/${trimmed}`;
}

/** 太大、名字不合法或同一批里重名的文件不送出。已存在的名字单独列出来等确认。 */
export function planUploads(dir: string, existingNames: readonly string[], files: readonly NamedSize[]): UploadPlan {
  const existing = new Set(existingNames);
  const seen = new Set<string>();
  const send: UploadItem[] = [];
  const skipped: UploadSkip[] = [];
  const overwrite: string[] = [];
  for (const file of files) {
    const label = file.name.trim() || '未命名';
    const path = remoteChildPath(dir, file.name);
    if (!path) {
      skipped.push({ name: label, reason: `${label} 的名字不能带斜杠或 ..` });
      continue;
    }
    if (seen.has(file.name)) {
      skipped.push({ name: file.name, reason: `${file.name} 在这一批里重名，已跳过` });
      continue;
    }
    seen.add(file.name);
    if (file.size > MAX_UPLOAD_BYTES) {
      skipped.push({ name: file.name, reason: `${file.name} 超过 64MB，没有开始上传` });
      continue;
    }
    if (existing.has(file.name)) overwrite.push(file.name);
    send.push({ name: file.name, path });
  }
  return { send, skipped, overwrite };
}

export interface DeletePrompt {
  title: string;
  body: string;
}

/** 符号链接只去掉链接。目录才递归。不能把指向目录的链接说成要删掉目标。 */
export function deletePrompt(kind: 'dir' | 'file' | 'symlink', name: string): DeletePrompt {
  if (kind === 'symlink') {
    return {
      title: `删除链接「${name}」？`,
      body: '只会去掉这个符号链接，不会删除它指向的文件或目录。',
    };
  }
  if (kind === 'dir') {
    return {
      title: `删除目录「${name}」？`,
      body: '这个目录会递归删除，里面的文件和子目录都会去掉，而且不能恢复。',
    };
  }
  return {
    title: `删除「${name}」？`,
    body: '这个文件会从远端删除，不能恢复。',
  };
}

/**
 * 正在传输时的进度文字。
 * total 为 0 表示大小未知。以前直接显示 100%，文件还在下时看起来已经完成。
 */
export function transferRunningText(transferred: number, total: number): string {
  if (total > 0) {
    const pct = Math.min(100, Math.round((transferred / total) * 100));
    return `${pct}%`;
  }
  return `已传输 ${transferred} 字节`;
}

/** 符号链接在列表里几乎总是 0777。权限框要说明改的是目标，默认值用目标的权限位。 */
export function chmodPrompt(kind: 'dir' | 'file' | 'symlink'): string {
  if (kind === 'symlink') {
    return '这会修改链接指向的文件，不是链接本身。输入它的八进制权限';
  }
  return '输入八进制权限 (例如 755 或 644)';
}

/** 三位或四位八进制。755、0644 可以，999 和空字符串不行。 */
export function parseOctalMode(raw: string): number | null {
  const text = raw.trim();
  if (!/^[0-7]{3,4}$/.test(text)) return null;
  return Number.parseInt(text, 8);
}
