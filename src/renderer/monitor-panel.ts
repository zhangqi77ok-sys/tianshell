import type { CommandSnippet, PublicHost, ServerMetrics } from '../shared/types.js';
import { webApi } from './api-client.js';
import { formatBytes } from './sftp-panel.js';

export interface MonitorPanelDeps {
  onSendCommand: (cmd: string) => void;
  getHosts: () => PublicHost[];
  toast: (msg: string) => void;
}

export class MonitorAndToolsController {
  private readonly cpuEl = document.getElementById('mon-cpu') as HTMLElement;
  private readonly cpuBar = document.getElementById('mon-cpu-bar') as HTMLElement;
  private readonly memEl = document.getElementById('mon-mem') as HTMLElement;
  private readonly memBar = document.getElementById('mon-mem-bar') as HTMLElement;
  private readonly swapEl = document.getElementById('mon-swap') as HTMLElement;
  private readonly swapBar = document.getElementById('mon-swap-bar') as HTMLElement;
  private readonly rxEl = document.getElementById('mon-rx') as HTMLElement;
  private readonly txEl = document.getElementById('mon-tx') as HTMLElement;
  private readonly loadEl = document.getElementById('mon-load') as HTMLElement;
  private readonly hostEl = document.getElementById('mon-host') as HTMLElement;
  private readonly disksEl = document.getElementById('mon-disks') as HTMLElement;
  private readonly procsEl = document.getElementById('mon-procs') as HTMLElement;
  private readonly snippetsEl = document.getElementById('snippet-list') as HTMLElement;

  constructor(private readonly deps: MonitorPanelDeps) {
    this.bindDialogs();
  }

  renderMetrics(metrics: ServerMetrics | null): void {
    if (!metrics) {
      this.hostEl.textContent = '未连接';
      this.cpuEl.textContent = '--%';
      this.cpuBar.style.width = '0%';
      this.memEl.textContent = '-- / --';
      this.memBar.style.width = '0%';
      this.swapEl.textContent = '-- / --';
      this.swapBar.style.width = '0%';
      this.rxEl.textContent = '0 B/s';
      this.txEl.textContent = '0 B/s';
      this.loadEl.textContent = '--';
      this.disksEl.textContent = '';
      this.procsEl.textContent = '';
      return;
    }
    this.hostEl.textContent = metrics.hostname;
    this.setBar(this.cpuBar, metrics.cpuPercent);
    this.cpuEl.textContent = `${metrics.cpuPercent.toFixed(1)}%`;

    const memPct = metrics.memTotalKb > 0 ? (metrics.memUsedKb / metrics.memTotalKb) * 100 : 0;
    this.setBar(this.memBar, memPct);
    this.memEl.textContent = `${formatBytes(metrics.memUsedKb * 1024)} / ${formatBytes(metrics.memTotalKb * 1024)}`;

    const swPct = metrics.swapTotalKb > 0 ? (metrics.swapUsedKb / metrics.swapTotalKb) * 100 : 0;
    this.setBar(this.swapBar, swPct);
    this.swapEl.textContent = metrics.swapTotalKb > 0
      ? `${formatBytes(metrics.swapUsedKb * 1024)} / ${formatBytes(metrics.swapTotalKb * 1024)}`
      : '未启用';

    this.rxEl.textContent = `${formatBytes(metrics.net.rxBytesPerSec)}/s`;
    this.txEl.textContent = `${formatBytes(metrics.net.txBytesPerSec)}/s`;
    this.loadEl.textContent = metrics.loadAvg.map((x) => x.toFixed(2)).join('  ');

    this.disksEl.textContent = '';
    for (const d of metrics.disks.slice(0, 4)) {
      const row = document.createElement('div');
      row.className = 'mono';
      row.textContent = `💽 ${d.mount} · ${d.usePercent}% (${formatBytes(d.usedKb * 1024)})`;
      this.disksEl.appendChild(row);
    }

    this.procsEl.textContent = '';
    for (const p of metrics.processes.slice(0, 5)) {
      const row = document.createElement('div');
      row.className = 'mono';
      row.textContent = `⚙ ${p.command} · CPU ${p.cpuPercent}% · MEM ${p.memPercent}%`;
      this.procsEl.appendChild(row);
    }
  }

  private setBar(bar: HTMLElement, pct: number): void {
    const safe = Math.max(0, Math.min(100, pct));
    bar.style.width = `${safe}%`;
    bar.classList.toggle('danger', safe >= 85);
  }

  async refreshSnippets(): Promise<void> {
    const list = await webApi.listSnippets();
    this.snippetsEl.textContent = '';
    for (const item of list) this.renderSnippetCard(item);
  }

  private renderSnippetCard(item: CommandSnippet): void {
    const card = document.createElement('div');
    card.className = 'snippet-card';
    const left = document.createElement('div');
    const title = document.createElement('div');
    title.textContent = `${item.name} [${item.category}]`;
    const cmd = document.createElement('div');
    cmd.className = 'mono host-meta';
    cmd.textContent = item.command;
    left.append(title, cmd);

    const del = document.createElement('button');
    del.type = 'button';
    del.textContent = '删';
    del.style.padding = '2px 6px';
    del.addEventListener('click', async (e) => {
      e.stopPropagation();
      await webApi.deleteSnippet(item.id);
      await this.refreshSnippets();
    });
    card.append(left, del);
    card.addEventListener('click', () => this.deps.onSendCommand(`${item.command}\n`));
    this.snippetsEl.appendChild(card);
  }

  async openTunnelModal(): Promise<void> {
    const dlg = document.getElementById('tunnel-dialog') as HTMLDialogElement;
    const hostSel = document.getElementById('tun-host') as HTMLSelectElement;
    hostSel.textContent = '';
    for (const h of this.deps.getHosts()) {
      const opt = document.createElement('option');
      opt.value = h.id;
      opt.textContent = `${h.alias} (${h.host})`;
      hostSel.appendChild(opt);
    }
    await this.renderTunnels();
    dlg.showModal();
  }

  private async renderTunnels(): Promise<void> {
    const rowsEl = document.getElementById('tunnel-rows') as HTMLElement;
    const runtimes = await webApi.listTunnels();
    rowsEl.textContent = '';
    if (!runtimes.length) {
      const empty = document.createElement('div');
      empty.className = 'empty-muted';
      empty.textContent = '暂无端口转发规则。支持 -L 本地转发、-R 远程映射、-D 动态 SOCKS5。';
      rowsEl.appendChild(empty);
      return;
    }
    for (const r of runtimes) {
      const row = document.createElement('div');
      row.className = 'tunnel-row';
      const info = document.createElement('div');
      info.className = 'mono';
      const dst = r.mode === 'socks5' ? 'SOCKS5 动态代理' : `${r.remoteHost}:${r.remotePort}`;
      info.textContent = `${r.active ? '🟢' : '⚪'} [${r.mode}] ${r.name} · ${r.localHost}:${r.localPort} → ${dst}`;
      const ops = document.createElement('div');
      ops.style.display = 'flex';
      ops.style.gap = '6px';
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.textContent = r.active ? '停止' : '启动';
      toggle.addEventListener('click', async () => {
        try {
          if (r.active) await webApi.stopTunnel(r.id);
          else await webApi.startTunnel(r.id);
          await this.renderTunnels();
        } catch (err) {
          this.deps.toast(err instanceof Error ? err.message : '隧道操作失败');
        }
      });
      const del = document.createElement('button');
      del.type = 'button';
      del.textContent = '删除';
      del.addEventListener('click', async () => {
        await webApi.deleteTunnel(r.id);
        await this.renderTunnels();
      });
      ops.append(toggle, del);
      row.append(info, ops);
      rowsEl.appendChild(row);
    }
  }

  private bindDialogs(): void {
    const snipDlg = document.getElementById('snippet-dialog') as HTMLDialogElement;
    document.getElementById('add-snippet')?.addEventListener('click', () => snipDlg.showModal());
    document.getElementById('cancel-snippet')?.addEventListener('click', () => snipDlg.close());
    document.getElementById('snippet-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      await webApi.saveSnippet({
        name: (document.getElementById('snip-name') as HTMLInputElement).value,
        category: (document.getElementById('snip-cat') as HTMLInputElement).value,
        command: (document.getElementById('snip-cmd') as HTMLTextAreaElement).value,
      });
      snipDlg.close();
      await this.refreshSnippets();
    });

    const tunDlg = document.getElementById('tunnel-dialog') as HTMLDialogElement;
    const tunMode = document.getElementById('tun-mode') as HTMLSelectElement;
    const syncMode = () => {
      const isSocks = tunMode.value === 'socks5';
      document.getElementById('tun-rhost-wrap')?.classList.toggle('hidden', isSocks);
      document.getElementById('tun-rport-wrap')?.classList.toggle('hidden', isSocks);
    };
    tunMode.addEventListener('change', syncMode);
    document.getElementById('close-tunnels')?.addEventListener('click', () => tunDlg.close());
    document.getElementById('tunnel-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      await webApi.saveTunnel({
        name: (document.getElementById('tun-name') as HTMLInputElement).value,
        hostId: (document.getElementById('tun-host') as HTMLSelectElement).value,
        mode: tunMode.value as 'local' | 'remote' | 'socks5',
        localHost: '127.0.0.1',
        localPort: Number((document.getElementById('tun-lport') as HTMLInputElement).value),
        remoteHost: (document.getElementById('tun-rhost') as HTMLInputElement).value,
        remotePort: Number((document.getElementById('tun-rport') as HTMLInputElement).value),
      });
      await this.renderTunnels();
    });
  }
}
