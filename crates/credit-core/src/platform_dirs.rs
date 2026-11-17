//! 跨平台应用数据目录探测（Windows / macOS / Linux）。
//! 各 IDE/Electron 客户端在不同 OS 下的用户数据目录约定集中在这里。

use std::path::PathBuf;

/// Electron userData 根目录（不含应用名）
pub fn electron_userdata_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(p) = std::env::var_os("APPDATA") {
            return PathBuf::from(p);
        }
        return dirs::home_dir().unwrap_or_default().join("AppData").join("Roaming");
    }
    #[cfg(target_os = "macos")]
    {
        // Electron 默认：~/Library/Application Support
        return dirs::home_dir().unwrap_or_default().join("Library").join("Application Support");
    }
    #[cfg(target_os = "linux")]
    {
        // Electron 默认：$XDG_CONFIG_HOME 或 ~/.config
        if let Ok(p) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(p);
        }
        return dirs::home_dir().unwrap_or_default().join(".config");
    }
}

/// Electron local 数据根（Windows LOCALAPPDATA 专用；mac/linux 与 userData 同）
pub fn electron_local_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(p);
        }
        return dirs::home_dir().unwrap_or_default().join("AppData").join("Local");
    }
    #[cfg(not(target_os = "windows"))]
    {
        electron_userdata_root()
    }
}

/// 在某个 Electron userData 根下找应用目录（跨平台名候选）
pub fn find_app_dir(root: &PathBuf, names: &[&str]) -> Option<PathBuf> {
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
