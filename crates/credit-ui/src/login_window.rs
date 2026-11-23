//! 登录授权小窗：Chrome/Edge `--app` 模式弹无地址栏小窗，登录终态自动关闭。
//!
//! 背景：原先用系统默认浏览器整页打开授权 URL，
//! ① `cmd /C start "" url` 未对 URL 加引号，URL 中的 `&` 被 cmd 当命令分隔符，
//!    授权参数全部丢失 → 授权页打不开/行为异常（所有平台无法登录的直接原因）；
//! ② 全尺寸浏览器窗口体验差。
//!
//! 方案：
//! - 找本机 Chrome → Edge，用 `--app=<url> --window-size=480,680` 开独立小窗
//!   （独立 `--user-data-dir` 指向临时目录，避免占用用户主配置、进程可独立 kill）；
//! - 返回窗口进程句柄；登录终态（ready/error/expired）调用 [`close_login_window`] kill 之；
//! - 找不到 Chrome/Edge 时退回系统默认浏览器（引号已修复）。

use std::path::PathBuf;
use std::sync::Mutex;

/// 各浏览器候选路径（按优先级）
fn browser_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    // Chrome 优先（兼容性最好），其次 Edge（Windows 自带，几乎必有）
    for p in [
        format!(r"{pf}\Google\Chrome\Application\chrome.exe"),
        format!(r"{pf86}\Google\Chrome\Application\chrome.exe"),
        format!(r"{local}\Google\Chrome\Application\chrome.exe"),
        format!(r"{pf86}\Microsoft\Edge\Application\msedge.exe"),
        format!(r"{pf}\Microsoft\Edge\Application\msedge.exe"),
    ] {
        out.push(PathBuf::from(p));
    }
    out
}

/// 当前登录窗口进程（同一时刻只应有一个登录窗）
static LOGIN_WINDOW: Mutex<Option<LoginWindow>> = Mutex::new(None);

struct LoginWindow {
    pid: u32,
    /// 窗口 data 目录（关闭后清理）
    data_dir: PathBuf,
}

/// 用 app 模式小窗打开登录授权页。
/// 返回 Ok(true) 表示用 Chrome/Edge 小窗打开；Ok(false) 表示退回系统默认浏览器。
pub fn open_login_window(url: &str, w: i32, h: i32) -> Result<bool, String> {
    // 先关掉可能残留的旧登录窗（上次登录没走到终态的场景）
    close_login_window();

    let exe = browser_candidates().into_iter().find(|p| p.exists());
    let Some(exe) = exe else {
        // 没有浏览器可用：退回系统默认（引号修复版）
        open_default_browser(url)?;
        return Ok(false);
    };
    let data_dir = std::env::temp_dir().join(format!("credit-login-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&data_dir);
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(format!("--app={url}"))
        .arg(format!("--window-size={w},{h}"))
        .arg(format!("--user-data-dir={}", data_dir.display()))
        // 首次运行的各种引导弹窗全部关掉，保证打开即登录页
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-features=ChromeWhatsNewUI")
        .arg("--window-position=200,120");
    let child = credit_core::proc_util::hide_console(cmd).spawn().map_err(|e| format!("启动浏览器失败: {e}"))?;
    let pid = child.id();
    // drop child 不杀进程（Chrome 会立刻派生子进程接管窗口，主进程可能退出）
    std::mem::forget(child);
    if let Ok(mut g) = LOGIN_WINDOW.lock() {
        *g = Some(LoginWindow { pid, data_dir: data_dir.clone() });
    }
    Ok(true)
}

/// 关闭登录小窗（登录终态 / 用户取消时调用）。没有窗口时静默。
pub fn close_login_window() {
    let win = {
        let mut g = LOGIN_WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        g.take()
    };
    let Some(w) = win else { return };
    // Chrome/Edge 主进程退出会带走窗口；kill 树更稳（taskkill /T）
    let _ = credit_core::proc_util::hide_console(std::process::Command::new("taskkill"))
        .args(["/PID", &w.pid.to_string(), "/T", "/F"])
        .status();
    // 等待文件句柄释放后清理临时 profile
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        let _ = std::fs::remove_dir_all(&w.data_dir);
    });
}

/// 系统默认浏览器打开（修复版：URL 加引号防 cmd 切断 `&`）。
fn open_default_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // `start "" "url"`：第一个空串是窗口标题占位，URL 必须整体加引号，
        // 否则查询串里的 & 会被 cmd 当作命令分隔符把 URL 截断。
        credit_core::proc_util::hide_console(std::process::Command::new("cmd"))
            .args(["/C", "start", "", url])
            .spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn().map_err(|e| e.to_string())?;
    Ok(())
}
