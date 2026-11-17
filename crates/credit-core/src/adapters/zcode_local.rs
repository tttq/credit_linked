//! ZCode（智谱）本机凭据扫描（原版 worker.mjs readZcodeCurrent 对齐）。
//! <home>/.zcode/v2/setting.json → dataBaseDir → <dataBaseDir>/.zcode/v2/credentials.json
//! 值加密 enc:v1:{iv}.{tag}.{data}（base64url 三段，AES-256-GCM，key=SHA256(secret)）。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::crypto::aes_gcm::gcm_decrypt;

pub const CREDENTIAL_FILENAME: &str = "credentials.json";
pub const TELEMETRY_FILENAME: &str = "telemetry-state.json";

fn fallback_secret() -> String {
    let username = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "unknown".into());
    format!(
        "zcode-credential-fallback:{}:{}:{}",
        std::env::consts::OS,
        dirs::home_dir().unwrap_or_default().display(),
        username
    )
}

/// enc:v1:{iv}.{tag}.{data} → 明文（非 enc: 前缀原样返回）
pub fn decode_zcode_value(raw: &str, secret: &str) -> Option<String> {
    if !raw.starts_with("enc:") {
        return Some(raw.to_string());
    }
    if !raw.starts_with("enc:v1:") {
        return None;
    }
    let parts: Vec<&str> = raw[7..].split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let iv = B64URL.decode(parts[0]).ok()?;
    let tag = B64URL.decode(parts[1]).ok()?;
    let data = B64URL.decode(parts[2]).ok()?;
    if iv.len() != 12 || tag.len() != 16 {
        return None;
    }
    let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
    let mut ct_tag = data;
    ct_tag.extend_from_slice(&tag);
    let pt = gcm_decrypt(&key, &iv, &ct_tag, b"").ok()?;
    String::from_utf8(pt).ok()
}

/// 解析 .zcode/v2 根目录（setting.json 的 dataBaseDir 或直接 home）
pub fn zcode_v2_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("ZCODE_HOME").ok()
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir());
    let home = home?;
    let v2 = home.join(".zcode").join("v2");
    let setting_file = v2.join("setting.json");
    if setting_file.exists() {
        // dataBaseDir 指向另一个根：<dataBaseDir>/.zcode/v2
        if let Ok(setting) = serde_json::from_reader::<_, Value>(std::fs::File::open(&setting_file).ok()?) {
            if let Some(base) = setting.get("dataBaseDir").and_then(|d| d.as_str()) {
                if std::path::Path::new(base).is_absolute() && !base.starts_with("\\\\") {
                    let alt = std::path::PathBuf::from(base).join(".zcode").join("v2");
                    if alt.join(CREDENTIAL_FILENAME).exists() {
                        return Some(alt);
                    }
                }
            }
        }
    }
    if v2.join(CREDENTIAL_FILENAME).exists() {
        return Some(v2);
    }
    None
}

pub struct ZcodeLiveAuth {
    pub uid: String,
    pub name: Option<String>,
    pub access_token: String,
    pub device_mid: String,
    pub version: Option<String>,
}

/// 读当前登录的 ZCode 凭据；未登录/未安装返回 None
pub fn read_live_auth() -> Option<ZcodeLiveAuth> {
    let dir = zcode_v2_dir()?;
    let cred: Value = serde_json::from_reader(std::fs::File::open(dir.join(CREDENTIAL_FILENAME)).ok()?).ok()?;
    let telemetry: Value = serde_json::from_reader(std::fs::File::open(dir.join(TELEMETRY_FILENAME)).ok()?).ok()?;
    let secret = std::env::var("ZCODE_CREDENTIAL_SECRET").unwrap_or_else(|_| fallback_secret());
    let read = |key: &str| -> Option<String> {
        let raw = cred.get(key)?.as_str()?;
        decode_zcode_value(raw, &secret)
    };
    // 仅支持 bigmodel 当前账号
    if read("oauth:active_provider")?.trim() != "bigmodel" {
        return None;
    }
    let user_info_raw = read("oauth:bigmodel:user_info")?;
    let user: Value = serde_json::from_str(&user_info_raw).ok()?;
    let uid = user.get("id").and_then(|i| i.as_str())?.to_string();
    let access_token = read("oauth:bigmodel:access_token")?;
    if access_token.is_empty() {
        return None;
    }
    let device_mid = telemetry.get("deviceMid").and_then(|d| d.as_str()).unwrap_or_default().to_string();
    let name = user.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
    Some(ZcodeLiveAuth { uid, name, access_token, device_mid, version: None })
}
