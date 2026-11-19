//! ZCode (Zhipu) 本机凭据扫描（跨平台）。
//! v1: enc:v1:{iv}.{tag}.{ct}，key=SHA256(fallback_secret)
//! v2: enc:v2:os:{fp}:{nonce}.{tag}.{ct}，key 来自 OS 密钥库：
//!     Windows=Credential Manager(PS1)；macOS=Keychain；Linux=libsecret（keyring crate）

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use crate::crypto::aes_gcm::gcm_decrypt;

/// 凭据文件名（~/.zcode/v2/ 下，值为 enc:v1/v2 加密串）
pub const CREDENTIAL_FILENAME: &str = "credentials.json";
/// 遥测文件名（读 deviceMid 用）
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
    // 客户端语义（逆向确认）：ZCODE_CREDENTIAL_SECRET / _FILE 互斥，key = SHA256(字符串原值)
    let env_secret = std::env::var("ZCODE_CREDENTIAL_SECRET").ok();
    let env_file = std::env::var("ZCODE_CREDENTIAL_SECRET_FILE").ok();
    if env_secret.is_some() && env_file.is_some() {
        return None; // 两者同时设置客户端直接报错
    }
    if let Some(secret) = env_secret {
        let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        return Some(key);
    }
    if let Some(path) = env_file {
        // 内容须为 32–4096 字节；key = SHA256(文件内容字符串)
        let s = std::fs::read_to_string(path).ok()?;
        let n = s.len();
        if !(32..=4096).contains(&n) { return None; }
        let key: [u8; 32] = Sha256::digest(s.as_bytes()).into();
        return Some(key);
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
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-File"]).arg(&script)
            .args(["-Target", &target]);
        let out = crate::proc_util::hide_console(cmd).output().ok()?;
        let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let key = B64URL.decode(b64.as_bytes()).ok()?;
        if key.len() != 32 { return None; }
        let mut r = [0u8; 32]; r.copy_from_slice(&key); Some(r)
    }
    #[cfg(not(windows))]
    {
        // 客户端在所有平台都用 SHA256(credentials.json 绝对路径)hex 作为 keyring user
        let abs = std::fs::canonicalize(cred_file).ok()?;
        let abs_str = abs.to_string_lossy().to_string();
        let abs_str = abs_str.strip_prefix("\\\\?\\").unwrap_or(&abs_str);
        let user = hex_lower(&Sha256::digest(abs_str.as_bytes()));
        let entry = keyring::Entry::new("ZCode Credential Encryption", &user).ok()?;
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
            // 格式 enc:v2:{os|env}:{fp24}:{iv}.{tag}.{ct}：fp = SHA256(key).hex()[0:24]，
            // 与当前解析出的 key 不匹配立即返回 None（客户端同语义，不做跨 key 尝试）
            let source = parts.get(2).copied()?;
            if source != "os" && source != "env" { return None; }
            let fp = parts.get(3).copied()?;
            let key = os_key?;
            let expect: String = Sha256::digest(key).iter().map(|b| format!("{b:02x}")).collect();
            if !expect.starts_with(fp) { return None; }
            decrypt_gcm(key, &B64URL.decode(seg[0]).ok()?, &B64URL.decode(seg[1]).ok()?, &B64URL.decode(seg[2]).ok()?)
        }
        _ => None,
    }
}

/// 定位 ~/.zcode/v2：ZCODE_HOME 覆盖 → setting.json dataBaseDir 兜底
pub fn zcode_v2_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("ZCODE_HOME").ok().map(std::path::PathBuf::from).or_else(dirs::home_dir)?;
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

/// 本机 ZCode 登录态（含 plan/billing 专用 zcodejwttoken）
pub struct ZcodeLiveAuth {
    pub uid: String,
    pub name: Option<String>,
    pub access_token: String,
    pub device_mid: String,
    /// plan/billing 类接口只认 zcodejwttoken（OAuth access_token 会 401）
    pub zcode_jwt: Option<String>,
}

/// 读取并解密单个凭据键（调试/补充 token 用，如 oauth:linked_bigmodel）
pub fn read_credential(key_name: &str) -> Option<String> {
    let dir = zcode_v2_dir()?;
    let cred_file = dir.join(CREDENTIAL_FILENAME);
    let cred: Value = serde_json::from_reader(std::fs::File::open(&cred_file).ok()?).ok()?;
    let key = os_key(&cred_file);
    let secret = fallback_secret();
    let raw = cred.get(key_name)?.as_str()?;
    decode_zcode_value(raw, &secret, key.as_ref())
}

/// 读取当前活跃 provider（zai_cn/zai/bigmodel）的本机凭据
pub fn read_live_auth() -> Option<ZcodeLiveAuth> {
    let dir = zcode_v2_dir()?;
    let cred_file = dir.join(CREDENTIAL_FILENAME);
    let cred: Value = serde_json::from_reader(std::fs::File::open(&cred_file).ok()?).ok()?;
    let key = os_key(&cred_file);
    let secret = fallback_secret();
    let read = |k: &str| -> Option<String> { decode_zcode_value(cred.get(k)?.as_str()?, &secret, key.as_ref()) };
    let provider = read("oauth:active_provider")?;
    let (uk, tk) = if provider == "zai_cn" { ("oauth:zai_cn:user_info", "oauth:zai_cn:access_token") }
        else if provider == "zai" { ("oauth:zai:user_info", "oauth:zai:access_token") }
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

/// token 指纹（识别客户端是否已刷新凭据；不泄露 token 明文）
pub fn token_fingerprint() -> Option<String> {
    let auth = read_live_auth()?;
    Some(hex_lower(&Sha256::digest(auth.access_token.as_bytes()))[..16].to_string())
}

/// 本机 ZCode 客户端可执行文件候选
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        "D:/ProgramFiles/develop/ZCode/ZCode.exe".to_string(),
        format!("{local}/Programs/ZCode/ZCode.exe"),
        format!("{local}/Programs/zcode/ZCode.exe"),
        "C:/Program Files/ZCode/ZCode.exe".to_string(),
        "D:/ProgramFiles/software/ZCode/ZCode.exe".to_string(),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机 ZCode 客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 启动本机 ZCode 客户端（已运行则单实例应用会自动聚焦现有窗口）。
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

/// 等待客户端刷新 token（credentials.json 中的 access_token 变化），最多 timeout。
/// 返回 Some(新 auth) = 已刷新；None = 超时/无法读取。
pub fn wait_for_token_change(old_fp: Option<&str>, timeout: std::time::Duration) -> Option<ZcodeLiveAuth> {
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