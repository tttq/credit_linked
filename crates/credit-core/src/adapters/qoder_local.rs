//! Qoder 本机凭据（auth.v1.dat "v10" GCM + Local State DPAPI）。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;

use crate::crypto::aes_gcm::decrypt_v10_blob;

/// 定位本机 Qoder userData 目录（国际/国内候选目录名不同）
pub fn app_dir(intl: bool) -> Option<std::path::PathBuf> {
    let base = crate::platform_dirs::electron_userdata_root();
    let candidates: &[&str] = if intl {
        &["com.qoder.app.stable", "com.qoder.app", "Qoder", "qoder"]
    } else {
        &["com.qodercn.app.stable", "com.qodercn.app", "Qoder CN", "qoder-cn"]
    };
    crate::platform_dirs::find_app_dir(&base, candidates)
}

/// Local State os_crypt.encrypted_key（"DPAPI" 前缀 5B）→ DPAPI → AES-256 key
pub fn load_aes_key(app: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    let ls: Value = serde_json::from_reader(std::fs::File::open(app.join("Local State"))?)?;
    let enc = ls["os_crypt"]["encrypted_key"].as_str().ok_or_else(|| anyhow::anyhow!("no encrypted_key"))?;
    let raw = B64.decode(enc)?;
    if raw.len() < 5 || &raw[..5] != b"DPAPI" {
        anyhow::bail!("not DPAPI prefixed");
    }
    let key = crate::vault::dpapi_unprotect(&raw[5..])?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&key);
    Ok(out)
}

/// 读取并解密 auth.v1.dat（"v10" GCM blob；AES key 来自 Local State DPAPI）
pub fn read_auth(intl: bool) -> anyhow::Result<Option<Value>> {
    let Some(app) = app_dir(intl) else { return Ok(None) };
    let dat = app.join("auth.v1.dat");
    if !dat.exists() {
        return Ok(None);
    }
    let blob = std::fs::read(&dat)?;
    let key = load_aes_key(&app)?;
    let pt = decrypt_v10_blob(&blob, &key, b"").map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Some(serde_json::from_slice(&pt)?))
}

/// token 指纹（识别 Qoder 客户端是否已刷新登录态；不泄露 token 明文）
pub fn token_fingerprint(intl: bool) -> Option<String> {
    use sha2::{Digest, Sha256};
    let v = read_auth(intl).ok()??;
    let token = v.get("token").and_then(|t| t.as_str()).unwrap_or("");
    if token.is_empty() { return None; }
    Some(Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string())
}

/// 本机 Qoder 客户端 exe 候选
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        format!("{local}/Programs/Qoder CN/Qoder CN.exe"),
        format!("{local}/Programs/Qoder/Qoder.exe"),
        format!("{local}/Programs/QoderWork/Qoder.exe"),
        "D:/ProgramFiles/develop/Qoder CN/Qoder CN.exe".to_string(),
        "C:/Program Files/Qoder/Qoder.exe".to_string(),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机 Qoder 客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 启动本机 Qoder 客户端（已运行则单实例应用聚焦现有窗口）。
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

/// 等待客户端刷新 token（指纹变化），最多 timeout。返回 Some = 已刷新（新 auth JSON）。
pub fn wait_for_token_change(intl: bool, old_fp: Option<&str>, timeout: std::time::Duration) -> Option<Value> {
    let start = std::time::Instant::now();
    loop {
        let cur_fp = token_fingerprint(intl);
        let changed = match (old_fp, cur_fp.as_deref()) {
            (Some(old), Some(cur)) => cur != old,
            (None, Some(_)) => true,
            _ => false,
        };
        if changed {
            return read_auth(intl).ok().flatten();
        }
        if start.elapsed() >= timeout { return None; }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}
