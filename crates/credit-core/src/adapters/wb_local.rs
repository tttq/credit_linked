//! WorkBuddy 本机会话读取（vendor/workbuddyLocal.mjs 对齐）。
//! 会话文件 %LOCALAPPDATA%/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info
//! （intl: workbuddy-desktop-ai.info）。$wbEncrypted 信封 AES-256-GCM，
//! 主密钥 = SHA256(atRestSecretKey)（spawn 官方客户端 ELECTRON_RUN_AS_NODE 取出）。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::crypto::aes_gcm::gcm_decrypt;

pub fn auth_dir() -> std::path::PathBuf {
    // Windows: %LOCALAPPDATA%/CodeBuddyExtension/Data/Public/auth
    // macOS:   ~/Library/Application Support/CodeBuddyExtension/Data/Public/auth
    // Linux:   ~/.config/CodeBuddyExtension/Data/Public/auth
    let base = crate::platform_dirs::electron_local_root();
    base.join("CodeBuddyExtension").join("Data").join("Public").join("auth")
}

pub fn session_file(intl: bool) -> std::path::PathBuf {
    auth_dir().join(if intl { "workbuddy-desktop-ai.info" } else { "workbuddy-desktop.info" })
}

fn is_wb_encrypted(v: &Value) -> bool {
    v.get("$wbEncrypted").and_then(|x| x.as_i64()) == Some(1)
        && v.get("envelope").and_then(|x| x.as_str()).is_some()
}

/// buildEnvelopeAAD：WB-AAD\0 || [1] || lp("WBEV1") || lp("sym-v1") || u32(suite) ||
/// lp(keyId) || [2,0,0]；lp = u32be(len)+bytes
pub fn build_envelope_aad(env: &Value) -> Vec<u8> {
    let lp = |s: &str| {
        let b = s.as_bytes();
        let mut out = (b.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(b);
        out
    };
    let u32v = |n: u32| n.to_be_bytes().to_vec();
    let mut out = Vec::new();
    out.extend_from_slice(b"WB-AAD\0");
    out.push(1);
    out.extend(lp("WBEV1"));
    out.extend(lp("sym-v1"));
    out.extend(u32v(env.get("suite").and_then(|s| s.as_u64()).unwrap_or(1) as u32));
    out.extend(lp(env.get("keyId").and_then(|k| k.as_str()).unwrap_or("")));
    out.extend_from_slice(&[2, 0, 0]);
    out
}

fn open_envelope(enc_obj: &Value, key: &[u8]) -> Option<Value> {
    let env_b64 = enc_obj.get("envelope")?.as_str()?;
    let env: Value = serde_json::from_slice(&B64.decode(env_b64).ok()?).ok()?;
    let nonce = B64.decode(env.get("nonce")?.as_str()?).ok()?;
    let ct = B64.decode(env.get("ciphertext")?.as_str()?).ok()?;
    let tag = B64.decode(env.get("authTag")?.as_str()?).ok()?;
    if nonce.len() != 12 || tag.len() != 16 {
        return None;
    }
    let mut ct_tag = ct;
    ct_tag.extend_from_slice(&tag);
    let pt = gcm_decrypt(key, &nonce, &ct_tag, &build_envelope_aad(&env)).ok()?;
    serde_json::from_slice(&pt).ok()
}

/// spawn 官方客户端取 atRestSecretKey（与原版逐字同脚本）。
/// client_exe 定位交给上层；这里只负责 spawn+哈希（跨平台 ELECTRON_RUN_AS_NODE）。
pub fn master_key_from_client(client_exe: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    const SCRIPT: &str = "const b=process._linkedBinding('electron_browser_workbuddy_storage');const p=JSON.parse(b.loggerGet());process.stdout.write(p.atRestSecretKey)";
    let out = std::process::Command::new(client_exe)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .arg("-e")
        .arg(SCRIPT)
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        anyhow::bail!("client binding script failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let h = Sha256::digest(&out.stdout);
    Ok(h.into())
}

/// 读会话 JSON（自动解 $wbEncrypted；key 缺失且无加密时直接读明文）
pub fn read_session(intl: bool, client_exe: Option<&std::path::Path>) -> anyhow::Result<Option<Value>> {
    let f = session_file(intl);
    if !f.exists() {
        return Ok(None);
    }
    // 登出标记检测
    let logged_out = std::path::PathBuf::from(format!("{}.logged-out", f.display()));
    if logged_out.exists() {
        return Ok(None);
    }
    let raw: Value = serde_json::from_reader(std::fs::File::open(&f)?)?;
    let session = if is_wb_encrypted(&raw) {
        let exe = client_exe.ok_or_else(|| anyhow::anyhow!("encrypted session requires client exe"))?;
        let key = master_key_from_client(exe)?;
        open_envelope(&raw, &key).ok_or_else(|| anyhow::anyhow!("envelope decrypt failed"))?
    } else {
        raw
    };
    Ok(Some(session))
}

/// sessionToAccount 精简对齐：取 auth.accessToken + account.uid/nickname/enterpriseId + domain
pub fn session_to_account(session: &Value) -> Option<Value> {
    let token = session.get("auth")?.get("accessToken")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let acc = session.get("account").cloned().unwrap_or(Value::Null);
    let auth = session.get("auth").cloned().unwrap_or(Value::Null);
    Some(serde_json::json!({
        "token": token,
        "refreshToken": auth.get("refreshToken").cloned().unwrap_or(Value::Null),
        "expiresAt": auth.get("expiresAt").cloned().unwrap_or(Value::Null),
        "uid": acc.get("uid").cloned().unwrap_or(Value::Null),
        "name": acc.get("nickname").or_else(|| acc.get("name")).cloned().unwrap_or(Value::Null),
        "domain": auth.get("domain").cloned().unwrap_or(Value::Null),
        "enterpriseId": acc.get("enterpriseId").cloned().unwrap_or(Value::Null),
    }))
}
