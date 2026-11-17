//! Trae 本机凭据扫描（vendor/traeLocal.mjs 对齐）。
//! 目录探测顺序：TRADE_HOME 覆盖 → %APPDATA%/Trae CN → %APPDATA%/TRAE SOLO CN。
//! 解析 storage.json 的 iCubeAuthInfo://icube.cloudide（tc 信封）与 icube-dc 设备号。

use std::path::PathBuf;

use serde_json::Value;

use crate::crypto::aes_cbc::tc_decrypt;

pub const APP_NAMES: [&str; 2] = ["Trae CN", "TRAE SOLO CN"];
pub const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";
pub const DC_PREFIX: &str = "iCubeAuthInfo://icube-dc:";

fn app_data_root() -> PathBuf {
    if let Some(p) = std::env::var_os("APPDATA") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join("AppData").join("Roaming")
}

pub fn trae_data_dir() -> Option<PathBuf> {
    if let Ok(over) = std::env::var("TRADE_HOME") {
        let p = PathBuf::from(&over);
        if p.join("User/globalStorage/storage.json").exists() {
            return Some(p);
        }
    }
    for name in APP_NAMES {
        let f = app_data_root().join(name).join("User/globalStorage/storage.json");
        if f.exists() {
            // dir = storage.json 的上三级（User/globalStorage/storage.json → app 根）
            return f.parent().and_then(|g| g.parent()).and_then(|u| u.parent()).map(|x| x.to_path_buf());
        }
    }
    None
}

fn read_storage_json(dir: &PathBuf) -> Option<Value> {
    let f = dir.join("User/globalStorage/storage.json");
    let text = std::fs::read_to_string(f).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).ok()
}

fn device_id_of(storage: &Value) -> Option<String> {
    let obj = storage.as_object()?;
    for key in obj.keys() {
        if let Some(id) = key.strip_prefix(DC_PREFIX) {
            if id.len() >= 8 && id.chars().all(|c| c.is_ascii_digit()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub struct TraeLiveAuth {
    pub data_dir: PathBuf,
    pub device_id: Option<String>,
    pub auth: Value,
    pub is_intl: bool,
    pub host: Option<String>,
}

/// 解出当前登录凭据；未登录/解不开返回 None。
pub fn read_live_auth() -> Option<TraeLiveAuth> {
    let dir = trae_data_dir()?;
    let storage = read_storage_json(&dir)?;
    let enc = storage.get(AUTH_KEY)?.as_str()?;
    let auth = tc_decrypt(enc)?;
    let token = auth.get("token")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    let host = auth.get("host").and_then(|h| h.as_str()).unwrap_or("").to_string();
    let is_intl = !host.is_empty() && !host_ends_with_cn(&host);
    Some(TraeLiveAuth { data_dir: dir, device_id: device_id_of(&storage), auth, is_intl, host: if host.is_empty() { None } else { Some(host) } })
}

fn host_ends_with_cn(host: &str) -> bool {
    // 与原版一致：/(^|\.)trae\.cn$/.test(hostname)
    let h = host.trim_start_matches("https://").trim_start_matches("http://");
    let h = h.split('/').next().unwrap_or(h);
    h == "trae.cn" || h.ends_with(".trae.cn")
}

/// SOLO 目录名含 "SOLO" → req_source=2，否则 1（对齐 readTraeCurrent）
pub fn request_source_for_dir(dir: &PathBuf) -> i64 {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.to_uppercase().contains("SOLO") { 2 } else { 1 }
}
