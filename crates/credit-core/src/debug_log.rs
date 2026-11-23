//! 调试日志工具 - GUI 应用使用文件日志而非控制台输出

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// 获取当前时间戳（秒）
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 写入调试日志到临时目录下的 credit-rs-debug.log
pub fn debug_log(module: &str, message: &str) {
    let log_file = std::env::temp_dir().join("credit-rs-debug.log");
    
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        let _ = writeln!(f, "[{} {}] {}", timestamp(), module, message);
    }
}

/// 写入错误日志
pub fn error_log(module: &str, message: &str) {
    let log_file = std::env::temp_dir().join("credit-rs-error.log");
    
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        let _ = writeln!(f, "[{} ERROR {}] {}", timestamp(), module, message);
    }
}
