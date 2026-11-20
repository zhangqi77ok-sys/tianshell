import type { PublicHost, ServerMetrics } from '../shared/types.js';
import { webApi } from './api-client.js';
import { historyAfterFailedRefresh, historyEmptyText, isCurrentGeneration } from './session-logic.js';
import { formatBytes } from './sftp-panel.js';

export interface MonitorPanelDeps {
  onFillCommand: (cmd: string) => void;
  getHosts: () => PublicHost[];
  toast: (msg: string, kind?: 'info' | 'error') => void;
  confirm: (title: string, body: string) => Promise<boolean>;
  reloadQuickOps?: () => void;
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
  private readonly diskReadEl = document.getElementById('mon-disk-read') as HTMLElement | null;
  private readonly diskWriteEl = document.getElementById('mon-disk-write') as HTMLElement | null;
  private readonly loadEl = document.getElementById('mon-load') as HTMLElement;
  private readonly loadDetailEl = document.getElementById('mon-load-detail') as HTMLElement | null;
  private readonly hostEl = document.getElementById('mon-host') as HTMLElement;
  private readonly disksEl = document.getElementById('mon-disks') as HTMLElement;
  private readonly procsEl = document.getElementById('mon-procs') as HTMLElement;
  private readonly historyEl = document.getElementById('history-list') as HTMLElement;
  private historyQuery = '';
  private historyTicket = 0;
  private historyReady = false;

  constructor(private readonly deps: MonitorPanelDeps) {
    this.bindDialogs();
  }

  private tunnelDialogOpen = false;

  renderMetrics(metrics: ServerMetrics | null, mode: 'idle' | 'sampling' | 'live' = metrics ? 'live' : 'idle'): void {
    const bento = document.getElementById('mon-bento');
    const note = document.getElementById('mon-note');
    const idle = mode !== 'live';
    bento?.classList.toggle('idle', idle);
    if (note) note.hidden = mode === 'live';
    if (mode === 'sampling' && note) note.textContent = '正在采样…';
    if (mode === 'idle' && note) note.textContent = '打开 SSH 会话后自动采样。';
    if (!metrics) {
      this.hostEl.textContent = mode === 'sampling' ? '采样中' : '空闲';
      this.cpuEl.textContent = '—';
      this.cpuBar.style.width = '0%';
      this.memEl.textContent = '—';
      this.memBar.style.width = '0%';
      this.swapEl.textContent = '—';
      this.swapBar.style.width = '0%';
      this.rxEl.textContent = '—';
      this.txEl.textContent = '—';
      if (this.diskReadEl) this.diskReadEl.textContent = '—';
      if (this.diskWriteEl) this.diskWriteEl.textContent = '—';
      this.loadEl.textContent = '—';
      if (this.loadDetailEl) this.loadDetailEl.textContent = '1m / 5m / 15m';
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
      : '—';

    this.rxEl.textContent = `${formatBytes(metrics.net.rxBytesPerSec)}/s`;
    this.txEl.textContent = `${formatBytes(metrics.net.txBytesPerSec)}/s`;

    if (this.diskReadEl) {
      this.diskReadEl.textContent = metrics.diskIo ? `${formatBytes(metrics.diskIo.readBytesPerSec)}/s` : '0 B/s';
    }
    if (this.diskWriteEl) {
      this.diskWriteEl.textContent = metrics.diskIo ? `${formatBytes(metrics.diskIo.writeBytesPerSec)}/s` : '0 B/s';
    }

    this.loadEl.textContent = metrics.loadAvg[0].toFixed(2);
    if (this.loadDetailEl) {
      this.loadDetailEl.textContent = `5m: ${metrics.loadAvg[1].toFixed(2)}  15m: ${metrics.loadAvg[2].toFixed(2)}`;
    }

    this.disksEl.textContent = '';
    for (const d of metrics.disks.slice(0, 4)) {
      const row = document.createElement('div');
      row.className = 'metric-line';
      const name = document.createElement('span');
      name.className = 'metric-name';
      name.textContent = d.mount;
      name.title = `${d.mount} · ${formatBytes(d.usedKb * 1024)}`;
      const num = document.createElement('span');
      num.className = 'metric-num mono';
      num.textContent = `${Math.round(d.usePercent)}%`;
      row.append(name, num);
      this.disksEl.appendChild(row);
    }

    this.procsEl.textContent = '';
    const head = document.createElement('div');
    head.className = 'proc-line proc-head';
    const headName = document.createElement('span');
    headName.className = 'metric-name';
    headName.textContent = '进程';
    const headCpu = document.createElement('span');
    headCpu.className = 'metric-num';
    headCpu.textContent = 'CPU';
    const headMem = document.createElement('span');
    headMem.className = 'metric-num';
    headMem.textContent = '内存';
    head.append(headName, headCpu, headMem);
    this.procsEl.appendChild(head);
    const processes = metrics.processes.slice(0, 8);
    if (!processes.length) {
      const empty = document.createElement('div');
      empty.className = 'empty-muted';
      empty.textContent = '这一拍没有进程数据。';
      this.procsEl.appendChild(empty);
      return;
    }
    for (const p of processes) {
      const row = document.createElement('div');
      row.className = 'proc-line';
      const name = document.createElement('span');
      name.className = 'metric-name';
      name.textContent = p.command;
      name.title = p.command;
      const cpu = document.createElement('span');
      cpu.className = 'metric-num mono';
      cpu.textContent = `${p.cpuPercent.toFixed(1)}%`;
      const mem = document.createElement('span');
      mem.className = 'metric-num mono';
      mem.textContent = `${p.memPercent.toFixed(1)}%`;
      row.append(name, cpu, mem);
      this.procsEl.appendChild(row);
    }
  }

  private setBar(bar: HTMLElement, pct: number): void {
    const safe = Math.max(0, Math.min(100, pct));
    bar.style.width = `${safe}%`;
    bar.classList.toggle('danger', safe >= 85);
  }

  async refreshHistory(): Promise<void> {
    if (!this.historyEl) return;
    const ticket = ++this.historyTicket;
    try {
      const rows = await webApi.listHistory(this.historyQuery);
      if (!isCurrentGeneration(ticket, this.historyTicket)) return;
      this.historyReady = true;
      this.historyEl.textContent = '';
      if (!rows.length) {
        this.showHistoryNote(historyEmptyText(this.historyQuery));
        return;
      }
      for (const row of rows) {
        const card = document.createElement('div');
        card.className = 'history-item';
        card.tabIndex = 0;
        card.title = '填入当前终端，不会直接执行';
        card.setAttribute('role', 'button');

        const meta = document.createElement('div');
        meta.className = 'history-meta';

        const metaLeft = document.createElement('div');
        metaLeft.className = 'history-meta-left';

        const timeSpan = document.createElement('span');
        timeSpan.className = 'history-time';
        timeSpan.textContent = formatHistoryTime(row.at);

        const whereSpan = document.createElement('span');
        whereSpan.className = 'history-tag';
        whereSpan.textContent = row.alias || (row.sessionKind === 'local' ? '本机' : row.sessionKind);

        metaLeft.append(timeSpan, whereSpan);

        const statusSpan = document.createElement('span');
        if (row.exitCode !== undefined) {
          statusSpan.className = `history-status ${row.exitCode === 0 ? 'ok' : 'err'}`;
          statusSpan.textContent = row.exitCode === 0 ? '成功' : `退出 ${row.exitCode}`;
        }

        meta.append(metaLeft, statusSpan);

        const copyBtn = document.createElement('button');
        copyBtn.type = 'button';
        copyBtn.className = 'history-copy-btn';
        copyBtn.title = '复制命令到剪贴板';
        copyBtn.textContent = '📋 复制';
        copyBtn.addEventListener('click', async (e) => {
          e.stopPropagation();
          try {
            await navigator.clipboard.writeText(row.command);
            this.deps.toast('已复制命令到剪贴板');
          } catch {
            this.deps.toast('复制失败', 'error');
          }
        });

        const cmd = document.createElement('div');
        cmd.className = 'history-cmd';
        cmd.textContent = row.command;

        card.append(meta, cmd, copyBtn);
        const fill = () => {
          card.classList.add('card-activated');
          setTimeout(() => card.classList.remove('card-activated'), 250);
          this.deps.onFillCommand(row.command);
        };
        card.addEventListener('click', fill);
        card.addEventListener('keydown', (event) => {
          if (event.key === 'Enter' && event.target === card) fill();
        });
        this.historyEl.appendChild(card);
      }
    } catch (err) {
      if (!isCurrentGeneration(ticket, this.historyTicket)) return;
      const message = err instanceof Error ? err.message : '命令历史没有读出来';
      if (historyAfterFailedRefresh(this.historyReady) === 'keep') {
        this.deps.toast(message, 'error');
        return;
      }
      this.showHistoryNote(message);
    }
  }

  private showHistoryNote(message: string): void {
    if (!this.historyEl) return;
    this.historyEl.textContent = '';
    const empty = document.createElement('div');
    empty.className = 'empty-muted';
    empty.textContent = message;
    this.historyEl.appendChild(empty);
  }

  refreshTunnelsIfOpen(): void {
    if (this.tunnelDialogOpen) void this.renderTunnels();
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
    this.tunnelDialogOpen = true;
    dlg.addEventListener('close', () => { this.tunnelDialogOpen = false; }, { once: true });
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
          this.deps.toast(err instanceof Error ? err.message : '隧道操作失败', 'error');
        }
      });
      const del = document.createElement('button');
      del.type = 'button';
      del.textContent = '删除';
      del.addEventListener('click', async () => {
        const ok = await this.deps.confirm(
          `删除隧道「${r.name}」？`,
          r.active ? '这条转发还在运行，删除后会停掉。' : '这条转发规则会从本机移除。',
        );
        if (!ok) return;
        try {
          await webApi.deleteTunnel(r.id);
          await this.renderTunnels();
        } catch (err) {
          this.deps.toast(err instanceof Error ? err.message : '删除隧道失败', 'error');
        }
      });
      ops.append(toggle, del);
      row.append(info, ops);
      rowsEl.appendChild(row);
    }
  }

  private bindDialogs(): void {
    const snipDlg = document.getElementById('snippet-dialog') as HTMLDialogElement;
    document.getElementById('history-refresh')?.addEventListener('click', () => void this.refreshHistory());
    document.getElementById('history-filter')?.addEventListener('input', (event) => {
      this.historyQuery = (event.target as HTMLInputElement).value;
      void this.refreshHistory();
    });
    document.getElementById('add-snippet')?.addEventListener('click', () => snipDlg.showModal());
    document.getElementById('cancel-snippet')?.addEventListener('click', () => snipDlg.close());
    document.getElementById('snippet-form')?.addEventListener('submit', async (e) => {
      e.preventDefault();
      try {
        await webApi.saveSnippet({
          name: (document.getElementById('snip-name') as HTMLInputElement).value,
          category: (document.getElementById('snip-cat') as HTMLInputElement).value,
          command: (document.getElementById('snip-cmd') as HTMLTextAreaElement).value,
        });
        snipDlg.close();
        this.deps.reloadQuickOps?.();
      } catch (err) {
        this.deps.toast(err instanceof Error ? err.message : '快捷命令没有保存', 'error');
      }
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
      try {
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
      } catch (err) {
        this.deps.toast(err instanceof Error ? err.message : '隧道规则没有保存', 'error');
      }
    });
  }
}

function formatHistoryTime(iso: string): string {
  try {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso.slice(5, 16).replace('T', ' ');
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    const h = String(d.getHours()).padStart(2, '0');
    const min = String(d.getMinutes()).padStart(2, '0');
    return `${m}-${day} ${h}:${min}`;
  } catch {
    return iso.slice(5, 16).replace('T', ' ');
  }
}
