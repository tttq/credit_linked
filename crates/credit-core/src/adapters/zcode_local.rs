//! ZCode (Zhipu) 本机凭据扫描（跨平台）。
//! v1: enc:v1:{iv}.{tag}.{ct}，key=SHA256(fallback_secret)
//! v2: enc:v2:os:{fp}:{nonce}.{tag}.{ct}，key 来自 OS 密钥库：
//!     Windows=Credential Manager(PS1)；macOS=Keychain；Linux=libsecret（keyring crate）

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use crate::crypto::aes_gcm::gcm_decrypt;

pub const CREDENTIAL_FILENAME: &str = "credentials.json";
pub const TELEMETRY_FILENAME: &str = "telemetry-state.json";

fn fallback_secret() -> String {
    format!("zcode-credential-fallback:{}:{}:{}", std::env::consts::OS,
        dirs::home_dir().unwrap_or_default().display(), crate::platform_dirs::username())
}
fn hex_lower(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }

fn decrypt_gcm(key: &[u8; 32], iv: &[u8], tag: &[u8], data: &[u8]) -> Option<String> {
    if iv.len() != 12 || tag.len() != 16 { return None; }
    let mut ct_tag = data.to_vec(); ct_tag.extend_from_slice(tag);
    String::from_utf8(gcm_decrypt(key, iv, &ct_tag, b"").ok()?).ok()
}

/// OS 密钥库读取 32 字节 key
fn os_key(cred_file: &std::path::Path) -> Option<[u8; 32]> {
    // 客户端支持的环境变量覆盖（app.asar gx/Xhe）
    if let Ok(secret) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        let key = B64URL.decode(secret.as_bytes()).ok()?;
        if key.len() == 32 { let mut r = [0u8; 32]; r.copy_from_slice(&key); return Some(r); }
    }
    if let Ok(path) = std::env::var("ZCODE_CREDENTIAL_SECRET_FILE") {
        let s = std::fs::read_to_string(path).ok()?;
        let key = B64URL.decode(s.trim().as_bytes()).ok()?;
        if key.len() == 32 { let mut r = [0u8; 32]; r.copy_from_slice(&key); return Some(r); }
    }
    #[cfg(windows)]
    {
        let abs = std::fs::canonicalize(cred_file).ok()?;
        let abs_str = abs.to_string_lossy().to_string();
        let abs_str = abs_str.strip_prefix("\\\\?\\").unwrap_or(&abs_str);
        let hash = hex_lower(&Sha256::digest(abs_str.as_bytes()));
        let target = format!("{}.ZCode Credential Encryption", hash);
        let candidates = [
            std::env::var("ZCODE_OSKEY_SCRIPT").ok().map(std::path::PathBuf::from),
            // 打包后：Tauri resource_dir / scripts
            std::env::var("RESOURCE_DIR").ok().map(|d| std::path::PathBuf::from(d).join("scripts/zcode-oskey.ps1")),
            std::env::current_exe().ok().and_then(|e| e.parent().map(|x| x.join("scripts/zcode-oskey.ps1"))),
            std::env::current_exe().ok().and_then(|e| e.parent().map(|x| x.join("../resources/scripts/zcode-oskey.ps1"))),
            std::env::current_dir().ok().map(|d| d.join("crates/credit-core/scripts/zcode-oskey.ps1")),
        ];
        let script = candidates.into_iter().flatten().find(|x| x.exists())?;
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-File"]).arg(&script)
            .args(["-Target", &target]).output().ok()?;
        let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let key = B64URL.decode(b64.as_bytes()).ok()?;
        if key.len() != 32 { return None; }
        let mut r = [0u8; 32]; r.copy_from_slice(&key); Some(r)
    }
    #[cfg(not(windows))]
    {
        let _ = cred_file;
        let entry = keyring::Entry::new("ZCode Credential Encryption", &crate::platform_dirs::username()).ok()?;
        let secret = entry.get_password().ok()?;
        let key = B64URL.decode(secret.as_bytes()).ok()?;
        if key.len() != 32 { return None; }
        let mut r = [0u8; 32]; r.copy_from_slice(&key); Some(r)
    }
}

/// 解密单条凭据值
pub fn decode_zcode_value(raw: &str, secret: &str, os_key: Option<&[u8; 32]>) -> Option<String> {
    if !raw.starts_with("enc:") { return Some(raw.to_string()); }
    let parts: Vec<&str> = raw.split(':').collect();
    let seg: Vec<&str> = parts.last()?.split('.').collect();
    if seg.len() != 3 { return None; }
    match parts.get(1).copied() {
        Some("v1") => {
            let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
            decrypt_gcm(&key, &B64URL.decode(seg[0]).ok()?, &B64URL.decode(seg[1]).ok()?, &B64URL.decode(seg[2]).ok()?)
        }
        Some("v2") => {
            let key = os_key?;
            decrypt_gcm(key, &B64URL.decode(seg[0]).ok()?, &B64URL.decode(seg[1]).ok()?, &B64URL.decode(seg[2]).ok()?)
        }
        _ => None,
    }
}

pub fn zcode_v2_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("ZCODE_HOME").ok().map(std::path::PathBuf::from).or_else(|| dirs::home_dir())?;
    let v2 = home.join(".zcode").join("v2");
    if v2.join(CREDENTIAL_FILENAME).exists() { return Some(v2); }
    let sf = v2.join("setting.json");
    if sf.exists() {
        if let Ok(v) = serde_json::from_reader::<_, Value>(std::fs::File::open(&sf).ok()?) {
            if let Some(base) = v.get("dataBaseDir").and_then(|d| d.as_str()) {
                if std::path::Path::new(base).is_absolute() {
                    let alt = std::path::PathBuf::from(base).join(".zcode").join("v2");
                    if alt.join(CREDENTIAL_FILENAME).exists() { return Some(alt); }
                }
            }
        }
    }
    None
}

pub struct ZcodeLiveAuth {
    pub uid: String,
    pub name: Option<String>,
    pub access_token: String,
    pub device_mid: String,
    /// plan/billing 类接口只认 zcodejwttoken（OAuth access_token 会 401）
    pub zcode_jwt: Option<String>,
}

pub fn read_live_auth() -> Option<ZcodeLiveAuth> {
    let dir = zcode_v2_dir()?;
    let cred_file = dir.join(CREDENTIAL_FILENAME);
    let cred: Value = serde_json::from_reader(std::fs::File::open(&cred_file).ok()?).ok()?;
    let key = os_key(&cred_file);
    let secret = fallback_secret();
    let read = |k: &str| -> Option<String> { decode_zcode_value(cred.get(k)?.as_str()?, &secret, key.as_ref()) };
    let provider = read("oauth:active_provider")?;
    let (uk, tk) = if provider == "zai_cn" { ("oauth:zai_cn:user_info", "oauth:zai_cn:access_token") }
        else if provider == "bigmodel" { ("oauth:bigmodel:user_info", "oauth:bigmodel:access_token") }
        else { return None; };
    let user: Value = serde_json::from_str(&read(uk)?).ok()?;
    let uid = user.get("user_id").or_else(|| user.get("id")).and_then(|x| x.as_str())?.to_string();
    let access_token = read(tk)?;
    if access_token.is_empty() { return None; }
    // zcodejwttoken：plan/billing 接口专用；可能缺失或为空
    let zcode_jwt = read("zcodejwttoken").filter(|s| !s.is_empty());
    let tele: Value = serde_json::from_reader(std::fs::File::open(dir.join(TELEMETRY_FILENAME)).ok()?).ok()?;
    let device_mid = tele.get("deviceMid").and_then(|d| d.as_str()).unwrap_or_default().to_string();
    let name = user.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
    Some(ZcodeLiveAuth { uid, name, access_token, device_mid, zcode_jwt })
}