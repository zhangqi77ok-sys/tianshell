import { createRequire } from 'node:module';
import type { DiskStat, ProcessStat, ServerMetrics } from '../shared/types.js';
import { execOnClient } from './client.js';

const require = createRequire(import.meta.url);
const { Client: SshClient } = require('ssh2') as typeof import('ssh2');
type SshClientInstance = InstanceType<typeof SshClient>;

export const MONITOR_PROBE_COMMAND = [
  'echo __HOST__; hostname 2>/dev/null || uname -n',
  'echo __UPTIME__; cat /proc/uptime 2>/dev/null || echo 0',
  'echo __LOAD__; cat /proc/loadavg 2>/dev/null || uptime',
  'echo __CPU__; head -n 1 /proc/stat 2>/dev/null || true',
  'echo __MEM__; cat /proc/meminfo 2>/dev/null || true',
  'echo __NET__; cat /proc/net/dev 2>/dev/null || true',
  'echo __DF__; df -kP 2>/dev/null || df -k 2>/dev/null || true',
  'echo __PS__; ps -eo pid,user,pcpu,pmem,comm --sort=-pcpu 2>/dev/null | head -n 9 || ps -eo pid,user,pcpu,pmem,comm 2>/dev/null | head -n 9 || true',
].join('; ');

export interface RawCounters {
  timestampMs: number;
  cpuTotal: number;
  cpuIdle: number;
  rxBytes: number;
  txBytes: number;
}

function splitSections(raw: string): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  let current = '';
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith('__') && trimmed.endsWith('__')) {
      current = trimmed.slice(2, -2);
      out[current] = [];
      continue;
    }
    if (current) out[current].push(line);
  }
  return out;
}

function parseCpu(lines: string[], prev?: RawCounters): { cpuPercent: number; cpuTotal: number; cpuIdle: number } {
  const line = lines.find((l) => l.trim().startsWith('cpu'));
  if (!line) return { cpuPercent: 0, cpuTotal: 0, cpuIdle: 0 };
  const nums = line.trim().split(/\s+/).slice(1).map((n) => Number(n) || 0);
  const idle = (nums[3] ?? 0) + (nums[4] ?? 0);
  const total = nums.reduce((sum, n) => sum + n, 0);
  if (prev && total > prev.cpuTotal) {
    const dTotal = total - prev.cpuTotal;
    const dIdle = Math.max(0, idle - prev.cpuIdle);
    return { cpuPercent: clampPct(((dTotal - dIdle) / dTotal) * 100), cpuTotal: total, cpuIdle: idle };
  }
  const pct = total > 0 ? clampPct(((total - idle) / total) * 100) : 0;
  return { cpuPercent: pct, cpuTotal: total, cpuIdle: idle };
}

function parseMem(lines: string[]): { memTotalKb: number; memUsedKb: number; swapTotalKb: number; swapUsedKb: number } {
  const kv = new Map<string, number>();
  for (const line of lines) {
    const m = /^([A-Za-z0-9_()]+):\s+(\d+)/.exec(line.trim());
    if (m) kv.set(m[1], Number(m[2]));
  }
  const memTotalKb = kv.get('MemTotal') ?? 0;
  const avail = kv.get('MemAvailable') ?? ((kv.get('MemFree') ?? 0) + (kv.get('Buffers') ?? 0) + (kv.get('Cached') ?? 0));
  const memUsedKb = Math.max(0, memTotalKb - avail);
  const swapTotalKb = kv.get('SwapTotal') ?? 0;
  const swapFreeKb = kv.get('SwapFree') ?? swapTotalKb;
  return { memTotalKb, memUsedKb, swapTotalKb, swapUsedKb: Math.max(0, swapTotalKb - swapFreeKb) };
}

function parseNet(lines: string[], prev: RawCounters | undefined, nowMs: number) {
  let totalRxBytes = 0;
  let totalTxBytes = 0;
  for (const line of lines) {
    const idx = line.indexOf(':');
    if (idx === -1) continue;
    const iface = line.slice(0, idx).trim();
    if (!iface || iface === 'lo') continue;
    const cols = line.slice(idx + 1).trim().split(/\s+/).map((n) => Number(n) || 0);
    totalRxBytes += cols[0] ?? 0;
    totalTxBytes += cols[8] ?? 0;
  }
  const elapsedSec = prev ? Math.max(0.2, (nowMs - prev.timestampMs) / 1000) : 0;
  const rxBytesPerSec = prev && elapsedSec > 0 ? Math.max(0, Math.round((totalRxBytes - prev.rxBytes) / elapsedSec)) : 0;
  const txBytesPerSec = prev && elapsedSec > 0 ? Math.max(0, Math.round((totalTxBytes - prev.txBytes) / elapsedSec)) : 0;
  return { totalRxBytes, totalTxBytes, rxBytesPerSec, txBytesPerSec };
}

function parseDisks(lines: string[]): DiskStat[] {
  const out: DiskStat[] = [];
  for (const line of lines.slice(1)) {
    const cols = line.trim().split(/\s+/);
    if (cols.length < 6) continue;
    const fs = cols[0];
    if (fs === 'tmpfs' || fs === 'devtmpfs' || fs.startsWith('/dev/loop')) continue;
    const totalKb = Number(cols[1]) || 0;
    const usedKb = Number(cols[2]) || 0;
    const availKb = Number(cols[3]) || 0;
    const usePercent = Number.parseFloat(cols[4].replace('%', '')) || 0;
    out.push({ filesystem: fs, totalKb, usedKb, availKb, usePercent, mount: cols.slice(5).join(' ') });
  }
  return out.slice(0, 10);
}

function parseProcesses(lines: string[]): ProcessStat[] {
  const out: ProcessStat[] = [];
  for (const line of lines.slice(1)) {
    const cols = line.trim().split(/\s+/);
    if (cols.length < 5) continue;
    const pid = Number(cols[0]);
    if (!Number.isInteger(pid)) continue;
    out.push({
      pid,
      user: cols[1],
      cpuPercent: Number(cols[2]) || 0,
      memPercent: Number(cols[3]) || 0,
      command: cols.slice(4).join(' '),
    });
  }
  return out.slice(0, 8);
}

export function parseMetricsSample(
  raw: string,
  prev?: RawCounters,
  nowMs = Date.now(),
): { metrics: ServerMetrics; counters: RawCounters } {
  const s = splitSections(raw);
  const hostname = (s.HOST?.[0] ?? 'unknown').trim() || 'unknown';
  const uptimeSeconds = Math.floor(Number((s.UPTIME?.[0] ?? '0').trim().split(/\s+/)[0]) || 0);
  const loadCols = (s.LOAD?.[0] ?? '').trim().split(/\s+/);
  const loadAvg: [number, number, number] = [Number(loadCols[0]) || 0, Number(loadCols[1]) || 0, Number(loadCols[2]) || 0];
  const cpu = parseCpu(s.CPU ?? [], prev);
  const mem = parseMem(s.MEM ?? []);
  const netStat = parseNet(s.NET ?? [], prev, nowMs);
  const metrics: ServerMetrics = {
    sampledAt: new Date(nowMs).toISOString(),
    hostname,
    uptimeSeconds,
    loadAvg,
    cpuPercent: cpu.cpuPercent,
    ...mem,
    disks: parseDisks(s.DF ?? []),
    net: netStat,
    processes: parseProcesses(s.PS ?? []),
  };
  return {
    metrics,
    counters: {
      timestampMs: nowMs,
      cpuTotal: cpu.cpuTotal,
      cpuIdle: cpu.cpuIdle,
      rxBytes: netStat.totalRxBytes,
      txBytes: netStat.totalTxBytes,
    },
  };
}

export async function sampleServerMetrics(
  client: SshClientInstance,
  prev?: RawCounters,
): Promise<{ metrics: ServerMetrics; counters: RawCounters }> {
  const res = await execOnClient(client, MONITOR_PROBE_COMMAND, 5_000);
  return parseMetricsSample(res.stdout, prev, Date.now());
}

function clampPct(v: number): number {
  return Math.round(Math.max(0, Math.min(100, v)) * 10) / 10;
}
