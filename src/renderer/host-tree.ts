import type { HostGroup, PublicHost } from '../shared/types.js';
import { webApi } from './api-client.js';

export interface HostUiState {
  connecting: boolean;
  cancelling: boolean;
  error: string | null;
  selected: boolean;
}

export interface HostTreeDeps {
  onOpenHost: (hostId: string) => void;
  onOpenAnother: (hostId: string) => void;
  onSelectHost?: (hostId: string) => void;
  onCancelConnect?: (hostId: string) => void;
  toast: (msg: string, kind?: 'info' | 'error') => void;
  promptText: (title: string, defaultVal?: string) => Promise<string | null>;
  confirm: (title: string, body: string) => Promise<boolean>;
  getActiveHostIds?: () => Set<string>;
  getHostUi?: (hostId: string) => HostUiState | undefined;
}

export class HostTreeController {
  hosts: PublicHost[] = [];
  groups: HostGroup[] = [];
  private editingHost: PublicHost | null = null;
  private readonly collapsed = new Set<string>();

  constructor(private readonly deps: HostTreeDeps) {
    this.bindEvents();
  }

  async refresh(): Promise<void> {
    const res = await webApi.getHosts();
    this.hosts = res.hosts;
    this.groups = res.groups;
    (document.getElementById('data-dir') as HTMLElement).textContent = res.dataDir;
    this.render();
  }

  render(): void {
    const q = (document.getElementById('filter') as HTMLInputElement).value.trim().toLowerCase();
    const tree = document.getElementById('host-tree') as HTMLElement;
    tree.textContent = '';
    if (!q && this.hosts.length === 0 && this.groups.length === 0) {
      const note = document.createElement('p');
      note.className = 'tree-empty';
      note.textContent = '还没有主机。用「+ 主机」添加，列表会跟着变。';
      tree.appendChild(note);
      return;
    }
    const filtered = this.hosts.filter(
      (h) => !q || h.alias.toLowerCase().includes(q) || h.host.includes(q) || (h.tags ?? []).some((t) => t.toLowerCase().includes(q)),
    );
    const buckets: Array<{ id: string | null; title: string; list: PublicHost[] }> = [
      ...this.groups.map((g) => ({ id: g.id, title: g.name, list: filtered.filter((h) => h.groupId === g.id) })),
      { id: null, title: '未分组主机', list: filtered.filter((h) => !h.groupId || !this.groups.some((g) => g.id === h.groupId)) },
    ];
    for (const bucket of buckets) {
      if (!bucket.list.length && bucket.id === null && this.groups.length > 0) continue;
      const key = bucket.id ?? '__none__';
      const hdr = document.createElement('div');
      hdr.className = 'group-hdr';
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'group-toggle';
      toggle.textContent = `${this.collapsed.has(key) ? '▸' : '▾'} ${bucket.title} (${bucket.list.length})`;
      toggle.addEventListener('click', () => {
        if (this.collapsed.has(key)) this.collapsed.delete(key);
        else this.collapsed.add(key);
        this.render();
      });
      hdr.appendChild(toggle);
      if (bucket.id) {
        const rename = document.createElement('button');
        rename.type = 'button';
        rename.className = 'group-rename';
        rename.textContent = '重命名';
        rename.setAttribute('aria-label', `重命名分组 ${bucket.title}`);
        const groupId = bucket.id;
        const currentName = bucket.title;
        rename.addEventListener('click', (event) => {
          event.stopPropagation();
          void this.renameGroup(groupId, currentName);
        });
        hdr.appendChild(rename);
      }
      tree.appendChild(hdr);
      if (this.collapsed.has(key)) continue;
      for (const h of bucket.list) tree.appendChild(this.renderHostRow(h));
    }
  }

  private renderHostRow(h: PublicHost): HTMLElement {
    const item = document.createElement('div');
    item.className = 'host-item';
    item.id = `host-${h.id}`;
    const ui = this.deps.getHostUi?.(h.id);
    const isOnline = Boolean(this.deps.getActiveHostIds?.().has(h.id));
    if (ui?.selected) item.classList.add('selected');
    if (ui?.connecting) item.classList.add('connecting');
    if (ui?.error) item.classList.add('failed');
    item.setAttribute('aria-selected', ui?.selected ? 'true' : 'false');
    const left = document.createElement('div');
    const alias = document.createElement('div');
    alias.className = 'host-alias';
    if (isOnline) {
      const dot = document.createElement('span');
      dot.className = 'online-dot';
      dot.title = '在线活跃连接中';
      alias.appendChild(dot);
    }
    const aliasText = document.createTextNode(`${h.jumpHostId ? '🔀 ' : ''}${h.alias}`);
    alias.appendChild(aliasText);
    const meta = document.createElement('div');
    meta.className = 'host-meta mono';
    meta.textContent = `${h.username}@${h.host}:${h.port}`;
    left.append(alias, meta);
    if (ui?.error) {
      const err = document.createElement('div');
      err.className = 'host-error';
      err.textContent = ui.error;
      left.append(err);
    }

    const ops = document.createElement('div');
    ops.className = 'host-actions';
    const connectBtn = document.createElement('button');
    connectBtn.type = 'button';
    connectBtn.className = 'host-connect-btn';
    if (ui?.cancelling) {
      connectBtn.textContent = '取消中';
      connectBtn.disabled = true;
    } else if (ui?.connecting) {
      connectBtn.textContent = '取消';
      connectBtn.title = '取消这次连接';
      connectBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.deps.onCancelConnect?.(h.id);
      });
    } else if (isOnline) {
      connectBtn.textContent = '切换';
      connectBtn.title = '切到这台主机已经打开的会话';
      connectBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.deps.onOpenHost(h.id);
      });
    } else {
      connectBtn.textContent = '连接';
      connectBtn.title = '连接此远程主机';
      connectBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.deps.onOpenHost(h.id);
      });
    }
    const editBtn = document.createElement('button');
    editBtn.type = 'button';
    editBtn.className = 'host-btn-icon';
    editBtn.textContent = '✏️';
    editBtn.title = '编辑主机配置';
    editBtn.setAttribute('aria-label', '编辑主机');
    editBtn.addEventListener('click', (e) => { e.stopPropagation(); this.openModal(h); });
    const delBtn = document.createElement('button');
    delBtn.type = 'button';
    delBtn.className = 'host-btn-icon';
    delBtn.textContent = '🗑️';
    delBtn.title = '删除主机';
    delBtn.setAttribute('aria-label', `删除主机 ${h.alias}`);
    delBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      const ok = await this.deps.confirm(
        `删除主机「${h.alias}」？`,
        '这台主机的配置会从本机移除，已保存的密码或私钥也会一起删掉。',
      );
      if (!ok) return;
      try {
        await webApi.deleteHost(h.id);
        await this.refresh();
      } catch (err) {
        this.deps.toast(err instanceof Error ? err.message : '删除主机失败', 'error');
      }
    });
    ops.append(connectBtn);
    if (isOnline && !ui?.connecting) {
      const again = document.createElement('button');
      again.type = 'button';
      again.className = 'host-again-btn';
      again.textContent = '再开';
      again.title = '再开一个 SSH 会话，当前标签保留';
      again.setAttribute('aria-label', `再开一个到 ${h.alias} 的会话`);
      again.addEventListener('click', (e) => {
        e.stopPropagation();
        this.deps.onOpenAnother(h.id);
      });
      ops.append(again);
    }
    ops.append(editBtn, delBtn);
    item.append(left, ops);
    item.addEventListener('click', () => this.deps.onSelectHost?.(h.id));
    item.addEventListener('dblclick', (event) => {
      if (event.shiftKey) this.deps.onOpenAnother(h.id);
      else this.deps.onOpenHost(h.id);
    });
    return item;
  }

  openModal(host: PublicHost | null): void {
    this.editingHost = host;
    const dlg = document.getElementById('host-dialog') as HTMLDialogElement;
    (document.getElementById('dialog-title') as HTMLElement).textContent = host ? `编辑 ${host.alias}` : '添加主机';
    this.fillSelect('field-group', '默认未分组', this.groups.map((g) => ({ value: g.id, label: g.name })));
    this.fillSelect(
      'field-jump',
      '直连 (无跳板机)',
      this.hosts.filter((c) => !host || c.id !== host.id).map((c) => ({ value: c.id, label: `${c.alias} (${c.host})` })),
    );
    (document.getElementById('field-alias') as HTMLInputElement).value = host?.alias ?? '';
    (document.getElementById('field-host') as HTMLInputElement).value = host?.host ?? '';
    (document.getElementById('field-port') as HTMLInputElement).value = String(host?.port ?? 22);
    (document.getElementById('field-user') as HTMLInputElement).value = host?.username ?? '';
    (document.getElementById('field-group') as HTMLSelectElement).value = host?.groupId ?? '';
    (document.getElementById('field-jump') as HTMLSelectElement).value = host?.jumpHostId ?? '';
    (document.getElementById('field-tags') as HTMLInputElement).value = (host?.tags ?? []).join(', ');
    (document.getElementById('field-notes') as HTMLInputElement).value = host?.notes ?? '';
    const password = document.getElementById('field-password') as HTMLInputElement;
    const key = document.getElementById('field-key') as HTMLTextAreaElement;
    const phrase = document.getElementById('field-passphrase') as HTMLInputElement;
    password.value = '';
    key.value = '';
    phrase.value = '';
    const pick = document.getElementById('pick-key-file') as HTMLInputElement | null;
    if (pick) pick.value = '';
    (document.getElementById('field-clear-fp') as HTMLInputElement).checked = false;
    password.placeholder = host?.authType === 'password' ? '留空则继续使用已保存的密码' : '登录密码';
    key.placeholder = host?.authType === 'privateKey'
      ? '留空则继续使用已保存的私钥'
      : '粘贴 OpenSSH ed25519、PKCS#8 或 PEM 私钥。加密私钥请填写下面的口令';
    phrase.placeholder = host?.hasPassphrase ? '留空则继续使用已保存的口令' : '私钥口令，没有就留空';
    this.setAuth(host?.authType ?? 'password');
    (document.getElementById('form-error') as HTMLElement).textContent = '';
    document.getElementById('clear-fp-row')?.classList.toggle('hidden', !host?.fingerprint);
    dlg.showModal();
  }

  private setAuth(auth: 'password' | 'privateKey'): void {
    document.querySelectorAll<HTMLInputElement>('input[name="auth"]').forEach((radio) => {
      radio.checked = radio.value === auth;
    });
    document.getElementById('password-field')?.classList.toggle('hidden', auth === 'privateKey');
    document.getElementById('key-field')?.classList.toggle('hidden', auth !== 'privateKey');
  }

  private fillSelect(id: string, defaultLabel: string, opts: Array<{ value: string; label: string }>): void {
    const sel = document.getElementById(id) as HTMLSelectElement;
    sel.textContent = '';
    const def = document.createElement('option');
    def.value = '';
    def.textContent = defaultLabel;
    sel.appendChild(def);
    for (const o of opts) {
      const el = document.createElement('option');
      el.value = o.value;
      el.textContent = o.label;
      sel.appendChild(el);
    }
  }

  private async renameGroup(groupId: string, currentName: string): Promise<void> {
    const name = await this.deps.promptText('重命名分组', currentName);
    if (!name || name === currentName) return;
    try {
      await webApi.saveGroup(name, groupId);
      await this.refresh();
    } catch (err) {
      this.deps.toast(err instanceof Error ? err.message : '重命名分组失败', 'error');
    }
  }

  private bindEvents(): void {
    document.getElementById('filter')?.addEventListener('input', () => this.render());
    document.getElementById('add-host')?.addEventListener('click', () => this.openModal(null));
    document.getElementById('cancel-host')?.addEventListener('click', () => (document.getElementById('host-dialog') as HTMLDialogElement).close());
    document.getElementById('add-group')?.addEventListener('click', async () => {
      const name = await this.deps.promptText('输入新分组名称');
      if (!name) return;
      try {
        await webApi.saveGroup(name);
        await this.refresh();
      } catch (err) {
        this.deps.toast(err instanceof Error ? err.message : '添加分组失败', 'error');
      }
    });
    document.querySelectorAll<HTMLInputElement>('input[name="auth"]').forEach((r) => r.addEventListener('change', () => {
      const isKey = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement).value === 'privateKey';
      document.getElementById('password-field')?.classList.toggle('hidden', isKey);
      document.getElementById('key-field')?.classList.toggle('hidden', !isKey);
    }));
    const pickFile = document.getElementById('pick-key-file') as HTMLInputElement;
    pickFile?.addEventListener('change', async () => {
      if (pickFile.files?.[0]) (document.getElementById('field-key') as HTMLTextAreaElement).value = await pickFile.files[0].text();
    });
    document.getElementById('host-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      const auth = (document.querySelector('input[name="auth"]:checked') as HTMLInputElement).value as 'password' | 'privateKey';
      try {
        await webApi.saveHost({
          id: this.editingHost?.id,
          alias: (document.getElementById('field-alias') as HTMLInputElement).value,
          host: (document.getElementById('field-host') as HTMLInputElement).value,
          port: Number((document.getElementById('field-port') as HTMLInputElement).value),
          username: (document.getElementById('field-user') as HTMLInputElement).value,
          authType: auth,
          secret: auth === 'password' ? (document.getElementById('field-password') as HTMLInputElement).value : (document.getElementById('field-key') as HTMLTextAreaElement).value,
          passphrase: (document.getElementById('field-passphrase') as HTMLInputElement).value,
          groupId: (document.getElementById('field-group') as HTMLSelectElement).value || null,
          jumpHostId: (document.getElementById('field-jump') as HTMLSelectElement).value || null,
          tags: (document.getElementById('field-tags') as HTMLInputElement).value.split(',').map((t) => t.trim()).filter(Boolean),
          notes: (document.getElementById('field-notes') as HTMLInputElement).value,
          clearFingerprint: (document.getElementById('field-clear-fp') as HTMLInputElement).checked,
        });
        (document.getElementById('host-dialog') as HTMLDialogElement).close();
        await this.refresh();
      } catch (err) {
        (document.getElementById('form-error') as HTMLElement).textContent = err instanceof Error ? err.message : '保存失败';
      }
    });
  }
}
