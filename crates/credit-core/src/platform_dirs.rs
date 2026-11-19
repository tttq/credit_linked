//! 跨平台应用数据目录探测（Windows / macOS / Linux）。
//! 使用运行时 target_os 判断，保证所有平台分支在任何工具链下都参与类型检查。

use std::path::PathBuf;

/// 当前 OS 名称（编译期常量，运行时判断）
const OS: &str = std::env::consts::OS;

/// Electron userData 根目录（不含应用名）
pub fn electron_userdata_root() -> PathBuf {
    match OS {
        "windows" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("AppData").join("Roaming")),
        "macos" => dirs::home_dir().unwrap_or_default().join("Library").join("Application Support"),
        _ => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config")),
    }
}

/// Electron local 数据根（Windows LOCALAPPDATA 专用；mac/linux 与 userData 同）
pub fn electron_local_root() -> PathBuf {
    match OS {
        "windows" => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("AppData").join("Local")),
        _ => electron_userdata_root(),
    }
}

/// 在某个 Electron userData 根下找应用目录（跨平台名候选）
pub fn find_app_dir(root: &std::path::Path, names: &[&str]) -> Option<PathBuf> {
    for name in names {
        let p = root.join(name);
        if p.exists() {
            return Some(p);
        }
        // Linux 常用小写/点分名
        let lower = root.join(name.to_lowercase().replace(' ', "-"));
        if lower.exists() {
            return Some(lower);
        }
    }
    None
}

/// 系统用户名的跨平台获取
pub fn username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into())
}
