//! MiniMax Code 本机凭据扫描（新版 CLI auth.json 明文格式）。
//! 路径: ~/.minimax/auth/prod/{cn|intl}/mcode-public/auth.json

use serde_json::Value;
use crate::adapters::Cred;

/// 本机 MiniMax Code 登录态（auth.json records 中首条有效记录）
pub struct MiniMaxLiveAuth { pub token: String, pub refresh_token: Option<String>, pub expires_at: Option<String>, pub region: String }

fn auth_dir() -> std::path::PathBuf {
    let home = std::env::var("MINIMAX_HOME").ok().map(std::path::PathBuf::from)
        .or_else(dirs::home_dir).unwrap_or_default();
    home.join(".minimax").join("auth").join("prod")
}

/// 读当前登录的 MiniMax Code 凭据（auth.json records 中第一条有效记录）
pub fn read_live_auth() -> Option<MiniMaxLiveAuth> {
    let root = auth_dir();
    for region in ["cn", "intl", "global"] {
        let dir = root.join(region).join("mcode-public");
        let file = dir.join("auth.json");
        if !file.exists() { continue; }
        let v: Value = serde_json::from_reader(std::fs::File::open(&file).ok()?).ok()?;
        let records = v.get("records")?.as_object()?;
        for (_key, rec) in records {
            let token = rec.get("accessToken")?.as_str()?.to_string();
            if token.is_empty() { continue; }
            let refresh = rec.get("refreshToken").and_then(|r| r.as_str()).map(|s| s.to_string());
            let expires_ms = rec.get("expiresAtMs").and_then(|e| e.as_u64()).unwrap_or(0);
            let expires = if expires_ms > 0 {
                // 简单 ISO（UTC）
                let secs = expires_ms / 1000;
                let days = (secs / 86400) as i64;
                let rem = secs % 86400;
                let (y, mo, d) = crate::adapters::civil_from_days(days);
                Some(format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, rem / 3600, (rem % 3600) / 60, rem % 60))
            } else { None };
            return Some(MiniMaxLiveAuth { token, refresh_token: refresh, expires_at: expires, region: region.to_string() });
        }
    }
    None
}

/// 直接转换为入库凭据
pub fn to_cred(auth: MiniMaxLiveAuth) -> Cred {
    Cred {
        provider: "minimax".into(),
        uid: String::new(),
        name: "MiniMax Code".into(),
        token: auth.token,
        refresh_token: auth.refresh_token,
        expires_at: auth.expires_at,
        meta: serde_json::json!({ "region": auth.region }),
    }
}

/// token 指纹（识别 MiniMax Code 客户端是否已刷新登录态；不泄露 token 明文）
pub fn token_fingerprint() -> Option<String> {
    use sha2::{Digest, Sha256};
    let auth = read_live_auth()?;
    if auth.token.is_empty() { return None; }
    Some(Sha256::digest(auth.token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string())
}

/// 本机 MiniMax Code 客户端 exe 候选
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        format!("{local}/Programs/minimax-code/MiniMax Code.exe"),
        format!("{local}/Programs/MiniMax Code/MiniMax Code.exe"),
        "D:/ProgramFiles/develop/MiniMax Code/MiniMax Code.exe".to_string(),
        format!("{local}/minimax-code/MiniMax Code.exe"),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机 MiniMax Code 客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 启动本机 MiniMax Code 客户端（已运行则单实例应用聚焦现有窗口）。
/// 返回 Some(路径) = 已尝试拉起；None = 未安装。
pub fn launch_client() -> Option<std::path::PathBuf> {
    let exe = find_client_exe()?;
    // GUI 应用：完全分离 stdio（否则子进程继承父进程管道句柄会导致父端读取挂起）
    let _ = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    Some(exe)
}

/// 等待客户端刷新 token（指纹变化），最多 timeout。返回 Some = 已刷新。
pub fn wait_for_token_change(old_fp: Option<&str>, timeout: std::time::Duration) -> Option<MiniMaxLiveAuth> {
    let start = std::time::Instant::now();
    loop {
        let cur_fp = token_fingerprint();
        let changed = match (old_fp, cur_fp.as_deref()) {
            (Some(old), Some(cur)) => cur != old,
            (None, Some(_)) => true,
            _ => false,
        };
        if changed {
            return read_live_auth();
        }
        if start.elapsed() >= timeout { return None; }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}