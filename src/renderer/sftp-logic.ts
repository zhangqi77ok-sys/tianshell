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

/** 三位或四位八进制。755、0644 可以，999 和空字符串不行。 */
export function parseOctalMode(raw: string): number | null {
  const text = raw.trim();
  if (!/^[0-7]{3,4}$/.test(text)) return null;
  return Number.parseInt(text, 8);
}
