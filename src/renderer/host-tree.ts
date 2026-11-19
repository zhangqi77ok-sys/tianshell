import type { HostGroup, PublicHost } from '../shared/types.js';
import { webApi } from './api-client.js';

export interface HostTreeDeps {
  onOpenHost: (hostId: string) => void;
  toast: (msg: string) => void;
  promptText: (title: string, defaultVal?: string) => Promise<string | null>;
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
      hdr.textContent = `${this.collapsed.has(key) ? '▸' : '▾'} ${bucket.title} (${bucket.list.length})`;
      hdr.addEventListener('click', () => {
        if (this.collapsed.has(key)) this.collapsed.delete(key);
        else this.collapsed.add(key);
        this.render();
      });
      tree.appendChild(hdr);
      if (this.collapsed.has(key)) continue;
      for (const h of bucket.list) tree.appendChild(this.renderHostRow(h));
    }
  }

  private renderHostRow(h: PublicHost): HTMLElement {
    const item = document.createElement('div');
    item.className = 'host-item';
    const left = document.createElement('div');
    const alias = document.createElement('div');
    alias.className = 'host-alias';
    alias.textContent = `${h.jumpHostId ? '🔀 ' : ''}${h.alias}`;
    const meta = document.createElement('div');
    meta.className = 'host-meta mono';
    meta.textContent = `${h.username}@${h.host}:${h.port}`;
    left.append(alias, meta);

    const ops = document.createElement('div');
    ops.className = 'host-actions';
    const editBtn = document.createElement('button');
    editBtn.type = 'button';
    editBtn.textContent = '改';
    editBtn.addEventListener('click', (e) => { e.stopPropagation(); this.openModal(h); });
    const delBtn = document.createElement('button');
    delBtn.type = 'button';
    delBtn.textContent = '删';
    delBtn.addEventListener('click', async (e) => {
      e.stopPropagation();
      await webApi.deleteHost(h.id);
      await this.refresh();
    });
    ops.append(editBtn, delBtn);
    item.append(left, ops);
    item.addEventListener('dblclick', () => this.deps.onOpenHost(h.id));
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
    (document.getElementById('field-password') as HTMLInputElement).value = '';
    (document.getElementById('field-key') as HTMLTextAreaElement).value = '';
    (document.getElementById('form-error') as HTMLElement).textContent = '';
    document.getElementById('clear-fp-row')?.classList.toggle('hidden', !host?.fingerprint);
    dlg.showModal();
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

  private bindEvents(): void {
    document.getElementById('filter')?.addEventListener('input', () => this.render());
    document.getElementById('add-host')?.addEventListener('click', () => this.openModal(null));
    document.getElementById('cancel-host')?.addEventListener('click', () => (document.getElementById('host-dialog') as HTMLDialogElement).close());
    document.getElementById('add-group')?.addEventListener('click', async () => {
      const name = await this.deps.promptText('输入新分组名称');
      if (name) { await webApi.saveGroup(name); await this.refresh(); }
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
