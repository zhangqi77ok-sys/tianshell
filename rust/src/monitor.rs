use std::collections::HashMap;
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

    ServerMetrics {
        sampled_at: chrono_now_iso(),
        hostname,
        uptime_seconds,
        load_avg,
        cpu_percent: 10.0,
        mem_total_kb,
        mem_used_kb,
        swap_total_kb,
        swap_used_kb,
        disks,
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
    let res = exec_command(sess, PROBE_COMMAND)?;
    Ok(parse_metrics_sample(&res.stdout))
}

fn chrono_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}Z", dur.as_secs(), dur.subsec_millis())
}
