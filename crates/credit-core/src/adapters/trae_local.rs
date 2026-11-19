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
    pub identity: TraeIdentity,
}

/// 请求头身份（对齐 dsh-connect-trae 的 readTraeIdentity）：
/// machineId = storage["telemetry.machineId"] ?? <userData>/machineid
/// deviceId  = iCubeAuthInfo://icube-dc:<id> ?? storage["telemetry.devDeviceId"] ?? sha256(machineId)[0..32]
/// buildVersion = storage["iCubeLastVersion"]；appVersion = 安装目录 product.json 的 appVersion
#[derive(Debug, Clone, Default)]
pub struct TraeIdentity {
    pub machine_id: String,
    pub device_id: String,
    pub app_version: Option<String>,
    pub build_version: Option<String>,
    pub os_version: String,
    pub device_cpu: Option<String>,
}

/// 从 userData 目录读取身份（storage.json + machineid + product.json）
pub fn read_identity(data_dir: &std::path::Path) -> Option<TraeIdentity> {
    let storage = read_storage_json(&data_dir.to_path_buf())?;
    let machine = storage.get("telemetry.machineId").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| s.to_string())
        .or_else(|| std::fs::read_to_string(data_dir.join("machineid")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    let machine_id = machine?;
    let device_id = device_id_of(&storage)
        .or_else(|| storage.get("telemetry.devDeviceId").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| {
            use sha2::{Digest, Sha256};
            let h = Sha256::digest(machine_id.as_bytes());
            h.iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_string()
        });
    let build_version = storage.get("iCubeLastVersion").and_then(|v| v.as_str()).map(|s| s.to_string());
    let app_version = find_product_app_version();
    Some(TraeIdentity {
        machine_id,
        device_id,
        app_version,
        build_version,
        os_version: os_version_string(),
        device_cpu: None,
    })
}

fn find_product_app_version() -> Option<String> {
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(l) = local { roots.push(l); }
    for r in ["C:/Program Files", "C:/Program Files (x86)", "D:/Program Files", "D:/ProgramFiles", "D:/ProgramFiles/develop", "D:/ProgramFiles/software"] {
        roots.push(PathBuf::from(r));
    }
    let names = ["Trae CN", "TRAE SOLO CN", "Trae", "Trae SOLO", "Trae CN/trae"];
    // 常见扁平安装（<root>/<name>/resources/app/product.json）
    for root in &roots {
        for name in names {
            let p = root.join(name).join("resources/app/product.json");
            if let Some(ver) = read_app_version(&p) { return Some(ver); }
            // 部分安装器多一层 Programs
            let p2 = root.join("Programs").join(name).join("resources/app/product.json");
            if let Some(ver) = read_app_version(&p2) { return Some(ver); }
        }
    }
    // 兜底：从运行中的 Trae 进程路径推导（<exe dir>/resources/app/product.json）
    let out = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Trae CN.exe", "/FO", "CSV", "/NH"])
        .output().ok()?;
    let _ = out;
    None
}

fn read_app_version(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    v.get("appVersion").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()).map(|s| s.to_string())
}

fn os_version_string() -> String {
    if cfg!(windows) {
        // 与插件同格式："Windows 10.0.22631"；用 cmd ver 拿不到内部版本号时退回 "Windows"
        let out = std::process::Command::new("cmd").args(["/C", "ver"]).output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout);
            if let Some(idx) = s.find("Version ") {
                let ver = s[idx + 8..].trim().trim_end_matches(']').trim();
                if !ver.is_empty() { return format!("Windows {ver}"); }
            }
        }
        "Windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macOS".to_string()
    } else {
        "Linux".to_string()
    }
}

/// Trae 安装自带的设备密钥对（storage.json 中 `iCubeAuthInfo://icube-dc:<id>` 的值）。
/// 该值用与 auth 相同的 tc 信封加密，明文是 `{"privateKeyPEM": "...", "publicKeyPEM": "..."}`。
/// ExchangeToken 刷新必须用私钥对设备证明签名（插件 refresh.ts 的 DeviceProof）。
pub struct TraeDeviceKeyPair {
    pub private_key_pem: String,
    pub public_key_pem: String,
}

pub fn read_device_keypair(data_dir: &std::path::Path) -> Option<TraeDeviceKeyPair> {
    let storage = read_storage_json(&data_dir.to_path_buf())?;
    let obj = storage.as_object()?;
    let key = obj.keys().find(|k| k.starts_with(DC_PREFIX))?;
    let raw = obj.get(key)?.as_str()?.trim().to_string();
    let v: Value = if raw.starts_with('{') { serde_json::from_str(&raw).ok()? } else { tc_decrypt(&raw)? };
    let private = v.get("privateKeyPEM").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())?.to_string();
    let public = v.get("publicKeyPEM").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())?.to_string();
    Some(TraeDeviceKeyPair { private_key_pem: private, public_key_pem: public })
}

/// 按官方客户端契约构造 DeviceProof：
/// message = "POST\n{path}\n{clientId}\n{refreshToken}\n{timestamp}\n{nonce}"（换行分隔，空格分隔会被拒 20403）
/// 设备私钥是 EC P-256（PKCS#8 PEM）；官方客户端用 `crypto.sign("sha256", msg, key)`，
/// 即 ECDSA-P256-SHA256 的 **DER** 签名，再 base64。
pub fn sign_device_proof(private_key_pem: &str, path: &str, client_id: &str, refresh_token: &str) -> Option<(String, i64, String)> {
    use base64::Engine;
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};
    use p256::pkcs8::DecodePrivateKey;

    let key = SigningKey::from_pkcs8_pem(private_key_pem).ok()?;
    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let nonce_bytes: [u8; 16] = rand::random();
    let nonce: String = nonce_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let message = format!("POST\n{path}\n{client_id}\n{refresh_token}\n{timestamp}\n{nonce}");
    let sig: Signature = key.sign(message.as_bytes());
    let der = sig.to_der();
    Some((base64::engine::general_purpose::STANDARD.encode(der.as_bytes()), timestamp, nonce))
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
    let identity = read_identity(&dir).unwrap_or_default();
    Some(TraeLiveAuth { data_dir: dir, device_id: device_id_of(&storage), auth, is_intl, host: if host.is_empty() { None } else { Some(host) }, identity })
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
