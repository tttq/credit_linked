//! 构建脚本：把 assets/app.ico 作为 Win32 资源嵌入 exe。
//!
//! 作用：资源管理器 / 任务栏 / 快捷方式都能拿到正确图标（gpui 窗口本身不设图标）。
//!
//! 注意：`winresource` 依赖 PATH 里的 `rc.exe`。Rust MSVC 工具链通常自带 Windows SDK，
//! 但 SDK 的 bin 目录不在 PATH 中，所以这里先自动定位并临时加入 PATH，再编译资源。
//! 找不到 rc.exe 时只打印警告，绝不阻断构建。

use std::path::PathBuf;

/// 在 Windows SDK 安装目录里找最新版本的 x64 rc.exe
#[cfg(windows)]
fn find_rc() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for base in [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ] {
        roots.push(PathBuf::from(base));
    }
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        let mut versions: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        // 版本目录倒序：优先最新 SDK
        versions.sort();
        versions.reverse();
        for v in versions {
            let rc = v.join("x64").join("rc.exe");
            if rc.exists() {
                return Some(rc);
            }
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        if let Some(rc) = find_rc() {
            if let Some(dir) = rc.parent() {
                let old = std::env::var("PATH").unwrap_or_default();
                std::env::set_var("PATH", format!("{};{}", dir.display(), old));
            }
        }

        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("ProductName", "AI Free Router");
        res.set("FileDescription", "AI Free Router");
        match res.compile() {
            Ok(()) => println!("cargo:warning=icon resource embedded OK"),
            Err(e) => println!("cargo:warning=icon resource FAILED: {e}"),
        }
    }
}
