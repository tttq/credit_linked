//! Trae 本机凭据扫描（vendor/traeLocal.mjs 对齐）。
//! 目录探测顺序：TRADE_HOME 覆盖 → %APPDATA%/Trae CN → %APPDATA%/TRAE SOLO CN。
//! 解析 storage.json 的 iCubeAuthInfo://icube.cloudide（tc 信封）与 icube-dc 设备号。

use std::path::PathBuf;

use serde_json::Value;

use crate::crypto::aes_cbc::tc_decrypt;

pub const APP_NAMES: [&str; 7] = [
    "Trae CN", "TRAE SOLO CN",              // Windows
    "Trae", "Trae CN", "Trae SOLO",          // macOS
    "trae", "trae-cn",                       // Linux
];
pub const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";
pub const DC_PREFIX: &str = "iCubeAuthInfo://icube-dc:";

fn app_data_root() -> PathBuf {
    crate::platform_dirs::electron_userdata_root()
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

/// 收集本机全部 Trae 安装目录下已注册的设备号（多安装/多目录去重）。
/// 上游 claim 校验 x-device-id 必须是服务器注册过的设备号（否则 9074），
/// 且一个设备号一天只能签一个账号（9095）——多账号需各用各的号。
pub fn collect_local_device_ids() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(over) = std::env::var("TRADE_HOME") {
        let p = PathBuf::from(&over);
        if p.join("User/globalStorage/storage.json").exists() {
            dirs.push(p);
        }
    }
    for name in APP_NAMES {
        let d = app_data_root().join(name);
        if d.join("User/globalStorage/storage.json").exists() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        if let Some(s) = read_storage_json(&d) {
            if let Some(id) = device_id_of(&s) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    out
}
