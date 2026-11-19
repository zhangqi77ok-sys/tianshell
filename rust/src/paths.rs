use std::path::PathBuf;

/// 解析 TianShell 的本地数据存储目录。
/// 优先级：环境变量 TIANSHELL_HOME > 系统标准 AppData 漫游目录。
pub fn resolve_data_dir() -> PathBuf {
    if let Ok(val) = std::env::var("TIANSHELL_HOME") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Some(config_dir) = dirs::config_dir() {
        return config_dir.join("tianshell");
    }
    PathBuf::from(".tianshell")
}
