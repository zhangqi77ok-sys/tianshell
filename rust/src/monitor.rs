use std::collections::HashMap;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use ssh2::Session;
use crate::ssh::{exec_command, SshError};

pub const PROBE_COMMAND: &str = "\
echo __HOST__; hostname 2>/dev/null || uname -n; \
echo __UPTIME__; cat /proc/uptime 2>/dev/null || echo 0; \
echo __LOAD__; cat /proc/loadavg 2>/dev/null || uptime; \
echo __CPU__; head -n 1 /proc/stat 2>/dev/null || true; \
echo __MEM__; cat /proc/meminfo 2>/dev/null || true; \
echo __NET__; cat /proc/net/dev 2>/dev/null || true; \
echo __DISKIO__; cat /proc/diskstats 2>/dev/null || true; \
echo __DF__; df -kP 2>/dev/null || df -k 2>/dev/null || true; \
echo __PS__; ps -eo pid,user,pcpu,pmem,comm --sort=-pcpu 2>/dev/null | head -n 9 || true";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskStat {
    pub filesystem: String,
    pub mount: String,
    #[serde(rename = "totalKb")]
    pub total_kb: u64,
    #[serde(rename = "usedKb")]
    pub used_kb: u64,
    #[serde(rename = "availKb")]
    pub avail_kb: u64,
    #[serde(rename = "usePercent")]
    pub use_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskIoStat {
    #[serde(rename = "readBytesPerSec")]
    pub read_bytes_per_sec: f64,
    #[serde(rename = "writeBytesPerSec")]
    pub write_bytes_per_sec: f64,
    #[serde(rename = "readIops")]
    pub read_iops: f64,
    #[serde(rename = "writeIops")]
    pub write_iops: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStat {
    pub pid: u32,
    pub user: String,
    #[serde(rename = "cpuPercent")]
    pub cpu_percent: f32,
    #[serde(rename = "memPercent")]
    pub mem_percent: f32,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetStat {
    #[serde(rename = "rxBytesPerSec")]
    pub rx_bytes_per_sec: f64,
    #[serde(rename = "txBytesPerSec")]
    pub tx_bytes_per_sec: f64,
    #[serde(rename = "totalRxBytes")]
    pub total_rx_bytes: u64,
    #[serde(rename = "totalTxBytes")]
    pub total_tx_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetrics {
    #[serde(rename = "sampledAt")]
    pub sampled_at: String,
    pub hostname: String,
    #[serde(rename = "uptimeSeconds")]
    pub uptime_seconds: u64,
    #[serde(rename = "loadAvg")]
    pub load_avg: [f32; 3],
    #[serde(rename = "cpuPercent")]
    pub cpu_percent: f32,
    #[serde(rename = "memTotalKb")]
    pub mem_total_kb: u64,
    #[serde(rename = "memUsedKb")]
    pub mem_used_kb: u64,
    #[serde(rename = "swapTotalKb")]
    pub swap_total_kb: u64,
    #[serde(rename = "swapUsedKb")]
    pub swap_used_kb: u64,
    pub disks: Vec<DiskStat>,
    pub net: NetStat,
    #[serde(rename = "diskIo")]
    pub disk_io: DiskIoStat,
    pub processes: Vec<ProcessStat>,
}

fn split_sections(raw: &str) -> HashMap<String, Vec<&str>> {
    let mut out: HashMap<String, Vec<&str>> = HashMap::new();
    let mut current = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("__") && trimmed.ends_with("__") && trimmed.len() > 4 {
            current = trimmed[2..trimmed.len() - 2].to_string();
            out.entry(current.clone()).or_default();
            continue;
        }
        if !current.is_empty() {
            out.entry(current.clone()).or_default().push(line);
        }
    }
    out
}

pub fn parse_metrics_sample(raw: &str) -> ServerMetrics {
    let sec = split_sections(raw);
    let hostname = sec.get("HOST").and_then(|v| v.first()).map(|s| s.trim()).unwrap_or("unknown").to_string();
    let uptime_seconds = sec.get("UPTIME").and_then(|v| v.first()).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<f32>().ok()).map(|f| f as u64).unwrap_or(0);
    let mut load_avg = [0.0f32; 3];
    if let Some(lines) = sec.get("LOAD") {
        if let Some(line) = lines.first() {
            let nums: Vec<f32> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            for (i, num) in nums.iter().take(3).enumerate() {
                load_avg[i] = *num;
            }
        }
    }
    let (mem_total_kb, mem_used_kb, swap_total_kb, swap_used_kb) = parse_mem(sec.get("MEM").unwrap_or(&Vec::new()));
    let disks = parse_df(sec.get("DF").unwrap_or(&Vec::new()));
    let processes = parse_ps(sec.get("PS").unwrap_or(&Vec::new()));
    let (total_rx, total_tx) = net_totals(&sec);

    ServerMetrics {
        sampled_at: crate::util::iso_now(),
        hostname,
        uptime_seconds,
        load_avg,
        cpu_percent: 0.0,
        mem_total_kb,
        mem_used_kb,
        swap_total_kb,
        swap_used_kb,
        disks,
        net: NetStat {
            rx_bytes_per_sec: 0.0,
            tx_bytes_per_sec: 0.0,
            total_rx_bytes: total_rx,
            total_tx_bytes: total_tx,
        },
        disk_io: DiskIoStat {
            read_bytes_per_sec: 0.0,
            write_bytes_per_sec: 0.0,
            read_iops: 0.0,
            write_iops: 0.0,
        },
        processes,
    }
}

fn parse_mem(lines: &[&str]) -> (u64, u64, u64, u64) {
    let mut map = HashMap::new();
    for line in lines {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() == 2 {
            let num = parts[1].trim().split_whitespace().next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            map.insert(parts[0].trim(), num);
        }
    }
    let total = map.get("MemTotal").copied().unwrap_or(0);
    let avail = map.get("MemAvailable").copied().unwrap_or(map.get("MemFree").copied().unwrap_or(0));
    let swap_total = map.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = map.get("SwapFree").copied().unwrap_or(swap_total);
    (total, total.saturating_sub(avail), swap_total, swap_total.saturating_sub(swap_free))
}

fn parse_df(lines: &[&str]) -> Vec<DiskStat> {
    let mut out = Vec::new();
    for line in lines.iter().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 6 && !cols[0].starts_with("tmpfs") {
            out.push(DiskStat {
                filesystem: cols[0].to_string(),
                mount: cols[5].to_string(),
                total_kb: cols[1].parse().unwrap_or(0),
                used_kb: cols[2].parse().unwrap_or(0),
                avail_kb: cols[3].parse().unwrap_or(0),
                use_percent: cols[4].trim_end_matches('%').parse().unwrap_or(0.0),
            });
        }
    }
    out
}

fn parse_ps(lines: &[&str]) -> Vec<ProcessStat> {
    let mut out = Vec::new();
    for line in lines.iter().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 5 {
            out.push(ProcessStat {
                pid: cols[0].parse().unwrap_or(0),
                user: cols[1].to_string(),
                cpu_percent: cols[2].parse().unwrap_or(0.0),
                mem_percent: cols[3].parse().unwrap_or(0.0),
                command: cols[4..].join(" "),
            });
        }
    }
    out
}

pub fn sample_remote_metrics(sess: &Session) -> Result<ServerMetrics, SshError> {
    let started = Instant::now();
    let first = exec_command(sess, PROBE_COMMAND)?;
    std::thread::sleep(Duration::from_millis(200));
    let second = exec_command(sess, PROBE_COMMAND)?;
    Ok(metrics_from_samples(&first.stdout, &second.stdout, started.elapsed().as_secs_f64()))
}

#[derive(Clone)]
pub(crate) struct ProbeHistory {
    pub raw: String,
    pub at: Instant,
}

/// Fold one probe into the previous sample. `exec` is `FnOnce`, so a caller
/// cannot run the remote script twice, and this function does not sleep.
/// CPU percent and interface rates stay at zero until a previous sample exists;
/// the gap is the time since that sample, not a fixed 200ms.
pub(crate) fn poll_probe(
    previous: &mut Option<ProbeHistory>,
    now: Instant,
    exec: impl FnOnce() -> Result<String, String>,
) -> Result<ServerMetrics, String> {
    let raw = exec()?;
    let metrics = match previous {
        Some(prev) => {
            let elapsed = now.saturating_duration_since(prev.at).as_secs_f64();
            metrics_from_samples(&prev.raw, &raw, elapsed)
        }
        None => parse_metrics_sample(&raw),
    };
    *previous = Some(ProbeHistory { raw, at: now });
    Ok(metrics)
}

pub fn metrics_from_samples(first_raw: &str, second_raw: &str, elapsed_secs: f64) -> ServerMetrics {
    let dt = elapsed_secs.max(0.05);
    let mut metrics = parse_metrics_sample(second_raw);
    if let (Some((t1, i1)), Some((t2, i2))) = (cpu_jiffies(first_raw), cpu_jiffies(second_raw)) {
        let total = t2.saturating_sub(t1);
        let idle = i2.saturating_sub(i1);
        if total > 0 {
            metrics.cpu_percent = (100.0 * (1.0 - idle as f32 / total as f32)).clamp(0.0, 100.0);
        }
    }
    let (rx1, tx1) = net_totals_raw(first_raw);
    let (rx2, tx2) = net_totals_raw(second_raw);
    metrics.net.rx_bytes_per_sec = rx2.saturating_sub(rx1) as f64 / dt;
    metrics.net.tx_bytes_per_sec = tx2.saturating_sub(tx1) as f64 / dt;
    metrics.net.total_rx_bytes = rx2;
    metrics.net.total_tx_bytes = tx2;

    let (r_sec1, w_sec1, r_op1, w_op1) = disk_io_totals_raw(first_raw);
    let (r_sec2, w_sec2, r_op2, w_op2) = disk_io_totals_raw(second_raw);
    metrics.disk_io.read_bytes_per_sec = (r_sec2.saturating_sub(r_sec1) * 512) as f64 / dt;
    metrics.disk_io.write_bytes_per_sec = (w_sec2.saturating_sub(w_sec1) * 512) as f64 / dt;
    metrics.disk_io.read_iops = r_op2.saturating_sub(r_op1) as f64 / dt;
    metrics.disk_io.write_iops = w_op2.saturating_sub(w_op1) as f64 / dt;
    metrics
}

fn cpu_jiffies(raw: &str) -> Option<(u64, u64)> {
    let sec = split_sections(raw);
    let line = sec.get("CPU")?.iter().find(|l| !l.trim().is_empty())?;
    let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|s| s.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total: u64 = nums.iter().sum();
    Some((total, idle))
}

fn net_totals(sec: &HashMap<String, Vec<&str>>) -> (u64, u64) {
    sum_net(sec.get("NET").map(Vec::as_slice).unwrap_or(&[]))
}

fn net_totals_raw(raw: &str) -> (u64, u64) {
    let sec = split_sections(raw);
    net_totals(&sec)
}

fn sum_net(lines: &[&str]) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in lines {
        let line = line.trim();
        if !line.contains(':') || line.contains('|') {
            continue;
        }
        let mut parts = line.split(':');
        let iface = parts.next().unwrap_or("").trim();
        if iface == "lo" || iface.is_empty() {
            continue;
        }
        let nums: Vec<u64> = parts.next().unwrap_or("").split_whitespace().filter_map(|s| s.parse().ok()).collect();
        if nums.len() >= 9 {
            rx = rx.saturating_add(nums[0]);
            tx = tx.saturating_add(nums[8]);
        }
    }
    (rx, tx)
}

fn is_disk_device(dev: &str) -> bool {
    if dev.starts_with("sd") || dev.starts_with("vd") {
        return dev.len() == 3 && dev.chars().nth(2).map(|c| c.is_ascii_lowercase()).unwrap_or(false);
    }
    if dev.starts_with("xvd") {
        return dev.len() == 4 && dev.chars().nth(3).map(|c| c.is_ascii_lowercase()).unwrap_or(false);
    }
    if dev.starts_with("nvme") {
        return dev.contains('n') && !dev.contains('p');
    }
    if dev.starts_with("mmcblk") {
        return !dev.contains('p');
    }
    false
}

fn disk_io_totals(sec: &HashMap<String, Vec<&str>>) -> (u64, u64, u64, u64) {
    sum_disk_io(sec.get("DISKIO").map(Vec::as_slice).unwrap_or(&[]))
}

fn disk_io_totals_raw(raw: &str) -> (u64, u64, u64, u64) {
    let sec = split_sections(raw);
    disk_io_totals(&sec)
}

fn sum_disk_io(lines: &[&str]) -> (u64, u64, u64, u64) {
    let mut total_r_sec = 0u64;
    let mut total_w_sec = 0u64;
    let mut total_r_ops = 0u64;
    let mut total_w_ops = 0u64;
    let mut matched_any = false;

    for line in lines {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 14 {
            let dev = cols[2];
            if is_disk_device(dev) {
                matched_any = true;
                let r_ops: u64 = cols[3].parse().unwrap_or(0);
                let r_sec: u64 = cols[5].parse().unwrap_or(0);
                let w_ops: u64 = cols[7].parse().unwrap_or(0);
                let w_sec: u64 = cols[9].parse().unwrap_or(0);
                total_r_ops = total_r_ops.saturating_add(r_ops);
                total_r_sec = total_r_sec.saturating_add(r_sec);
                total_w_ops = total_w_ops.saturating_add(w_ops);
                total_w_sec = total_w_sec.saturating_add(w_sec);
            }
        }
    }

    if !matched_any {
        for line in lines {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 14 {
                let dev = cols[2];
                if !dev.starts_with("loop") && !dev.starts_with("ram") && !dev.starts_with("sr") {
                    let r_ops: u64 = cols[3].parse().unwrap_or(0);
                    let r_sec: u64 = cols[5].parse().unwrap_or(0);
                    let w_ops: u64 = cols[7].parse().unwrap_or(0);
                    let w_sec: u64 = cols[9].parse().unwrap_or(0);
                    total_r_ops = total_r_ops.saturating_add(r_ops);
                    total_r_sec = total_r_sec.saturating_add(r_sec);
                    total_w_ops = total_w_ops.saturating_add(w_ops);
                    total_w_sec = total_w_sec.saturating_add(w_sec);
                }
            }
        }
    }

    (total_r_sec, total_w_sec, total_r_ops, total_w_ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_A: &str = "\
__HOST__
host-a
__UPTIME__
10.0 0
__LOAD__
0.10 0.20 0.30 1/2 3
__CPU__
cpu 100 0 0 400 0 0 0 0
__MEM__
MemTotal: 1000 kB
MemAvailable: 400 kB
SwapTotal: 100 kB
SwapFree: 100 kB
__NET__
eth0: 1000 0 0 0 0 0 0 0 500 0 0 0 0 0 0 0
__DISKIO__
   8       0 sda 100 0 2000 100 50 0 1000 50 0 100 150 0 0 0 0 0
__DF__
Filesystem 1K-blocks Used Available Use% Mounted on
/dev/sda1 1000 100 900 10% /
__PS__
PID USER %CPU %MEM COMMAND
1 root 0.1 0.1 init
";

    const SAMPLE_B: &str = "\
__HOST__
host-a
__UPTIME__
11.0 0
__LOAD__
0.40 0.20 0.30 1/2 3
__CPU__
cpu 180 0 0 420 0 0 0 0
__MEM__
MemTotal: 1000 kB
MemAvailable: 400 kB
SwapTotal: 100 kB
SwapFree: 100 kB
__NET__
eth0: 3000 0 0 0 0 0 0 0 2500 0 0 0 0 0 0 0
__DISKIO__
   8       0 sda 200 0 6000 200 150 0 3000 150 0 200 350 0 0 0 0
__DF__
Filesystem 1K-blocks Used Available Use% Mounted on
/dev/sda1 1000 100 900 10% /
__PS__
PID USER %CPU %MEM COMMAND
1 root 0.1 0.1 init
";

    #[test]
    fn one_probe_per_sample_uses_previous_gap_and_does_not_sleep() {
        let started = Instant::now();
        let mut calls = 0;
        let mut previous = None;
        let t0 = Instant::now();
        let first = poll_probe(&mut previous, t0, || {
            calls += 1;
            Ok(SAMPLE_A.to_string())
        })
        .expect("first probe");
        assert_eq!(calls, 1);
        assert_eq!(first.cpu_percent, 0.0);
        assert_eq!(first.net.rx_bytes_per_sec, 0.0);
        assert_eq!(first.net.tx_bytes_per_sec, 0.0);
        assert_eq!(first.hostname, "host-a");
        assert_eq!(first.mem_total_kb, 1000);
        assert_eq!(first.mem_used_kb, 600);
        assert_eq!(first.disks.len(), 1);
        assert_eq!(first.disks[0].mount, "/");
        assert_eq!(first.processes.len(), 1);

        let second = poll_probe(&mut previous, t0 + Duration::from_secs(1), || {
            calls += 1;
            Ok(SAMPLE_B.to_string())
        })
        .expect("second probe");
        assert_eq!(calls, 2, "each interactive sample runs the probe once");
        assert!((second.cpu_percent - 80.0).abs() < 0.05, "cpu {}", second.cpu_percent);
        assert!((second.net.rx_bytes_per_sec - 2000.0).abs() < 0.05, "rx {}", second.net.rx_bytes_per_sec);
        assert!((second.net.tx_bytes_per_sec - 2000.0).abs() < 0.05, "tx {}", second.net.tx_bytes_per_sec);
        assert!((second.disk_io.read_bytes_per_sec - 2048000.0).abs() < 1.0, "disk read {}", second.disk_io.read_bytes_per_sec);
        assert!((second.disk_io.write_bytes_per_sec - 1024000.0).abs() < 1.0, "disk write {}", second.disk_io.write_bytes_per_sec);
        assert!((second.disk_io.read_iops - 100.0).abs() < 0.1, "read iops {}", second.disk_io.read_iops);
        assert!((second.disk_io.write_iops - 100.0).abs() < 0.1, "write iops {}", second.disk_io.write_iops);
        assert!(started.elapsed() < Duration::from_millis(50), "probe fold slept: {:?}", started.elapsed());
    }
}
