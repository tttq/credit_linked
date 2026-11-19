//! CodeArts（华为码道）本机会话扫描（对齐客户端 FileSecretStorageService 逆向）。
//!
//! 真实存储模型（desktop 0.2.9 实测）：
//! - 目录：`%APPDATA%\codearts`（desktop；IDE 版为 codearts-agent 变体）
//! - 会话文件：`secretStorage\<sha256("authSession:cn-north-4")>.enc`
//!   内容 = base64( "v10" || nonce[12] || ct || tag[16] )（Chromium os_crypt，无 AAD）
//! - AES-256 key：`Local State` → `os_crypt.encrypted_key`（"DPAPI" 前缀 5B）→ DPAPI
//! - 明文 JSON：`session{ userId, userName, accessKey, secretKey, securityToken,
//!   refreshToken, expiresAt(ms) }` + `loginContext.pkcePair.codeVerifier`
//!   + `loginContext.dpopKeyPair.privateKey`（JWK 字符串）
//! - 客户端每次 refresh 都回写该文件 → 磁盘 RT 几乎总是最新可用的一条

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::crypto::aes_gcm::decrypt_v10_blob;

/// 候选数据目录（desktop: `codearts`；IDE/agent: `codearts-agent` 变体）
pub fn candidate_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let names = ["codearts", "CodeArts", "codearts-agent", "CodeArts Agent", "codearts_agent"];
    for base in [std::env::var("APPDATA"), std::env::var("LOCALAPPDATA")].into_iter().flatten() {
        for n in names {
            let d = std::path::PathBuf::from(&base).join(n);
            if d.join("Local State").exists() && !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
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

/// 解密单个 .enc 文件（base64 → v10 blob → GCM）→ session JSON
fn decrypt_enc_file(path: &std::path::Path, key: &[u8; 32]) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let blob = B64.decode(text.trim()).ok()?;
    let pt = decrypt_v10_blob(&blob, key, b"").ok()?;
    serde_json::from_slice(&pt).ok()
}

/// 判定是否码道会话 JSON（含 session.accessKey）
fn is_session_json(v: &Value) -> bool {
    v.pointer("/session/accessKey").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
}

/// 读客户端会话：优先精确哈希文件名，失败则遍历 secretStorage/*.enc
fn read_secret_storage(app: &std::path::Path, key: &[u8; 32]) -> Option<Value> {
    let dir = app.join("secretStorage");
    if !dir.is_dir() { return None; }
    // 精确：sha256("authSession:cn-north-4")
    let hashed: String = Sha256::digest(b"authSession:cn-north-4").iter().map(|b| format!("{b:02x}")).collect();
    if let Some(v) = decrypt_enc_file(&dir.join(format!("{hashed}.enc")), key) {
        if is_session_json(&v) { return Some(v); }
    }
    // 兜底：逐个尝试（region/版本变化）
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("enc") { continue; }
        if let Some(v) = decrypt_enc_file(&p, key) {
            if is_session_json(&v) { return Some(v); }
        }
    }
    None
}

/// 解出的本机码道登录信息
pub struct CodeArtsLive {
    pub desktop: bool,
    pub session: Value,
}

/// 读指定版本（desktop/agent）的本机码道会话
pub fn read_live_session(desktop: bool) -> Option<CodeArtsLive> {
    for dir in candidate_dirs() {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
        let is_desktop_dir = name == "codearts";
        if desktop != is_desktop_dir { continue; }
        let Ok(key) = load_aes_key(&dir) else { continue };
        if let Some(session) = read_secret_storage(&dir, &key) {
            return Some(CodeArtsLive { desktop, session });
        }
    }
    None
}

/// 会话指纹（识别客户端是否刷新了 STS；不泄露明文）
pub fn session_fingerprint(desktop: bool) -> Option<String> {
    let live = read_live_session(desktop).or_else(|| read_live_session(!desktop))?;
    let s = &live.session;
    let ak = s.pointer("/session/accessKey").and_then(|v| v.as_str()).unwrap_or("");
    let st = s.pointer("/session/securityToken").and_then(|v| v.as_str()).unwrap_or("");
    let mut h = Sha256::new();
    h.update(ak.as_bytes());
    h.update(st.as_bytes());
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string())
}

/// 本机码道客户端 exe 候选（desktop 与 IDE 版）
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        "D:/ProgramFiles/software/CodeArts/CodeArts.exe".to_string(),
        format!("{local}/Programs/CodeArts/CodeArts.exe"),
        "C:/Program Files/CodeArts/CodeArts.exe".to_string(),
        "C:/Program Files (x86)/CodeArts/CodeArts.exe".to_string(),
        "D:/ProgramFiles/develop/CodeArts/CodeArts.exe".to_string(),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机码道客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 启动本机码道客户端；返回 Some(路径) = 已尝试拉起；None = 未安装
pub fn launch_client() -> Option<std::path::PathBuf> {
    let exe = find_client_exe()?;
    // GUI 应用：完全分离 stdio（避免子进程继承父进程管道句柄导致挂起）
    let _ = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    Some(exe)
}

/// 等待客户端刷新 STS（会话指纹变化），最多 timeout
pub fn wait_for_session_change(desktop: bool, old_fp: Option<&str>, timeout: std::time::Duration) -> Option<CodeArtsLive> {
    let start = std::time::Instant::now();
    loop {
        let cur_fp = session_fingerprint(desktop);
        let changed = match (old_fp, cur_fp.as_deref()) {
            (Some(old), Some(cur)) => cur != old,
            (None, Some(_)) => true,
            _ => false,
        };
        if changed {
            return read_live_session(desktop).or_else(|| read_live_session(!desktop));
        }
        if start.elapsed() >= timeout { return None; }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}
