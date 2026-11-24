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
    
    // 优先使用环境变量（备用方案）
    if let Some(secret) = env_secret {
        eprintln!("[ZCode] Using ZCODE_CREDENTIAL_SECRET for decryption");
        let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        return Some(key);
    }
    if let Some(path) = env_file {
        // 内容须为 32–4096 字节；key = SHA256(文件内容字符串)
        let s = std::fs::read_to_string(path).ok()?;
        let n = s.len();
        if !(32..=4096).contains(&n) { 
            eprintln!("[ZCode] Invalid ZCODE_CREDENTIAL_SECRET_FILE length: {}", n);
            return None; 
        }
        eprintln!("[ZCode] Using ZCODE_CREDENTIAL_SECRET_FILE for decryption");
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
        
        eprintln!("[ZCode] Looking for credential encryption key, target: {}", target);
        
        let candidates = [
            std::env::var("ZCODE_OSKEY_SCRIPT").ok().map(std::path::PathBuf::from),
            // 打包后：Tauri resource_dir / scripts
            std::env::var("RESOURCE_DIR").ok().map(|d| std::path::PathBuf::from(d).join("scripts/zcode-oskey.ps1")),
            std::env::current_exe().ok().and_then(|e| e.parent().map(|x| x.join("scripts/zcode-oskey.ps1"))),
            std::env::current_exe().ok().and_then(|e| e.parent().map(|x| x.join("../resources/scripts/zcode-oskey.ps1"))),
            std::env::current_dir().ok().map(|d| d.join("crates/credit-core/scripts/zcode-oskey.ps1")),
        ];
        
        let script = candidates.into_iter().flatten().find(|x| {
            let exists = x.exists();
            if exists {
                eprintln!("[ZCode] Found script candidate: {:?}", x);
            }
            exists
        })?;
        
        eprintln!("[ZCode] Executing PowerShell script: {:?}", script);
        
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-File"]).arg(&script)
            .args(["-Target", &target]);
            
        let out = crate::proc_util::hide_console(cmd).output();
        
        match &out {
            Ok(out) => {
                if !out.status.success() {
                    eprintln!("[ZCode] PowerShell script failed with status: {}", out.status);
                    if !out.stderr.is_empty() {
                        eprintln!("[ZCode] Script stderr: {}", String::from_utf8_lossy(&out.stderr));
                    }
                }
            }
            Err(e) => {
                eprintln!("[ZCode] Failed to execute PowerShell script: {}", e);
            }
        }
        
        let out = out.ok()?;
        let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        
        if b64.is_empty() {
            eprintln!("[ZCode] ERROR: PowerShell script returned empty output");
            eprintln!("[ZCode] TIP: Check that ZCode client is logged in and credentials exist in Windows Credential Manager");
            eprintln!("[ZCode] TIP: Or set ZCODE_CREDENTIAL_SECRET environment variable as workaround");
            return None;
        }
        
        let key = B64URL.decode(b64.as_bytes()).ok()?;
        if key.len() != 32 { 
            eprintln!("[ZCode] ERROR: Decoded key length is {} (expected 32)", key.len());
            return None; 
        }
        
        eprintln!("[ZCode] Successfully retrieved encryption key from Windows Credential Manager");
        let mut r = [0u8; 32]; 
        r.copy_from_slice(&key); 
        Some(r)
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
        let mut r = [u8; 32]; r.copy_from_slice(&key); Some(r)
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
    // Try multiple possible locations for ZCode data directory
    
    // 1. ZCODE_HOME environment variable
    if let Ok(zcode_home) = std::env::var("ZCODE_HOME") {
        let path = std::path::PathBuf::from(&zcode_home);
        let v2 = path.join(".zcode").join("v2");
        if v2.join(CREDENTIAL_FILENAME).exists() {
            eprintln!("[ZCode] Found v2 dir via ZCODE_HOME: {:?}", v2);
            return Some(v2);
        }
    }
    
    // 2. APPDATA (Windows Electron app data)
    if let Ok(appdata) = std::env::var("APPDATA") {
        let candidates = vec![
            std::path::PathBuf::from(&appdata).join("ZCode"),
            std::path::PathBuf::from(&appdata).join("zcode"),
            std::path::PathBuf::from(&appdata).join("BigModel"),
            std::path::PathBuf::from(&appdata).join("bigmodel"),
        ];
        
        for base in candidates {
            let v2 = base.join(".zcode").join("v2");
            if v2.join(CREDENTIAL_FILENAME).exists() {
                eprintln!("[ZCode] Found v2 dir via APPDATA: {:?}", v2);
                return Some(v2);
            }
            
            // Also try direct .zcode in APPDATA
            let direct = base.join(".zcode");
            if direct.join(CREDENTIAL_FILENAME).exists() {
                eprintln!("[ZCode] Found .zcode dir via APPDATA: {:?}", direct);
                return Some(direct);
            }
        }
    }
    
    // 3. Home directory (~/.zcode)
    if let Some(home) = dirs::home_dir() {
        let v2 = home.join(".zcode").join("v2");
        if v2.join(CREDENTIAL_FILENAME).exists() {
            eprintln!("[ZCode] Found v2 dir via HOME: {:?}", v2);
            return Some(v2);
        }
        
        // Also try direct .zcode in home
        let direct = home.join(".zcode");
        if direct.join(CREDENTIAL_FILENAME).exists() {
            eprintln!("[ZCode] Found .zcode dir via HOME: {:?}", direct);
            return Some(direct);
        }
    }
    
    // 4. Check setting.json for dataBaseDir override
    if let Some(home) = dirs::home_dir() {
        let sf = home.join(".zcode").join("v2").join("setting.json");
        if sf.exists() {
            if let Ok(v) = serde_json::from_reader::<_, Value>(std::fs::File::open(&sf).ok()?) {
                if let Some(base) = v.get("dataBaseDir").and_then(|d| d.as_str()) {
                    if std::path::Path::new(base).is_absolute() {
                        let alt = std::path::PathBuf::from(base).join(".zcode").join("v2");
                        if alt.join(CREDENTIAL_FILENAME).exists() {
                            eprintln!("[ZCode] Found v2 dir via setting.json: {:?}", alt);
                            return Some(alt);
                        }
                    }
                }
            }
        }
    }
    
    eprintln!("[ZCode] No valid credential directory found");
    None
}

/// 本机 ZCode 登录态（含 plan/billing 专用 zcodejwttoken）
#[derive(Debug, Clone)]
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
    
    eprintln!("[ZCode] Reading credentials from: {:?}", cred_file);
    
    // 检查文件是否存在
    if !cred_file.exists() {
        eprintln!("[ZCode] ERROR: credentials.json not found at {:?}", cred_file);
        return None;
    }
    
    let cred: Value = serde_json::from_reader(std::fs::File::open(&cred_file).ok()?).ok()?;
    let key = os_key(&cred_file);
    
    if key.is_none() {
        eprintln!("[ZCode] ERROR: Failed to read encryption key");
        eprintln!("[ZCode] TIP: Ensure ZCode client is logged in");
        eprintln!("[ZCode] TIP: Check Windows Credential Manager for 'ZCode Credential Encryption' entries");
        eprintln!("[ZCode] TIP: Or set ZCODE_CREDENTIAL_SECRET environment variable as workaround");
        return None;
    }
    
    let secret = fallback_secret();
    
    let read = |k: &str| -> Option<String> { 
        let result = decode_zcode_value(cred.get(k)?.as_str()?, &secret, key.as_ref());
        if result.is_none() {
            eprintln!("[ZCode] Failed to decode credential: {}", k);
        }
        result
    };
    
    let provider = read("oauth:active_provider")?;
    eprintln!("[ZCode] Active provider: {}", provider);
    
    let (uk, tk) = if provider == "zai_cn" { ("oauth:zai_cn:user_info", "oauth:zai_cn:access_token") }
        else if provider == "zai" { ("oauth:zai:user_info", "oauth:zai:access_token") }
        else if provider == "bigmodel" { ("oauth:bigmodel:user_info", "oauth:bigmodel:access_token") }
        else { 
            eprintln!("[ZCode] Unsupported provider: {}. Supported: zai_cn, zai, bigmodel", provider);
            return None; 
        };
    
    let user_raw = read(uk)?;
    eprintln!("[ZCode] Decoded user info");
    
    let user: Value = serde_json::from_str(&user_raw).ok()?;
    let uid = user.get("user_id").or_else(|| user.get("id")).and_then(|x| x.as_str())?.to_string();
    let access_token = read(tk)?;
    
    eprintln!("[ZCode] UID: {}, Token length: {}", uid, access_token.len());
    
    if access_token.is_empty() { 
        eprintln!("[ZCode] Access token is empty");
        return None; 
    }
    
    // zcodejwttoken：plan/billing 接口专用；可能缺失或为空
    let zcode_jwt = read("zcodejwttoken").filter(|s| !s.is_empty());
    
    // Read telemetry for deviceMid
    let tele_file = dir.join(TELEMETRY_FILENAME);
    eprintln!("[ZCode] Looking for telemetry file: {:?}", tele_file);
    
    if !tele_file.exists() {
        eprintln!("[ZCode] WARNING: telemetry-state.json not found, using default deviceMid");
    }
    
    let tele: Value = serde_json::from_reader(std::fs::File::open(tele_file).ok()?).ok()?;
    let device_mid = tele.get("deviceMid").and_then(|d| d.as_str()).unwrap_or_default().to_string();
    
    let name = user.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
    
    eprintln!("[ZCode] Successfully loaded auth for user: {} (UID: {})", 
               name.as_deref().unwrap_or("unknown"), uid);
    
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
/// 本机 ZCode 客户端版本（用于请求头与 app_version 参数）。
///
/// 硬编码版本号是**致命**的：客户端升级后服务端会以 `app_version too old`
/// 拒绝 preview/claim/balance 与模型请求，表现为「整个 ZCode 都用不了」。
/// 探测顺序：环境变量 → exe 文件版本信息 → 安装目录里的 package.json → 目录名中的版本号 → 兜底。
pub fn client_version() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        if let Ok(v) = std::env::var("ZCODE_APP_VERSION") {
            let v = v.trim().to_string();
            if !v.is_empty() { return v; }
        }
        
        let Some(exe) = find_client_exe() else { return "3.18.2".into() };
        
        // 优先从 exe 文件版本信息读取（适用于 4.0.0+）
        if let Some(version) = read_exe_version(&exe) {
            eprintln!("[ZCode] Detected version from exe: {}", version);
            return version;
        }
        
        let root = exe.parent().map(|p| p.to_path_buf());
        if let Some(root) = root {
            // Electron 打包：resources/app/package.json
            for rel in ["resources/app/package.json", "package.json"] {
                let p = root.join(rel);
                if let Ok(s) = std::fs::read_to_string(&p) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                            if ver.split('.').count() >= 2 { 
                                eprintln!("[ZCode] Detected version from package.json: {}", ver);
                                return ver.to_string(); 
                            }
                        }
                    }
                }
            }
            // 便携版常见布局：<root>/<版本号>/ZCode.exe
            if let Some(name) = root.file_name().and_then(|n| n.to_str()) {
                if name.split('.').count() >= 3 && name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    eprintln!("[ZCode] Detected version from directory name: {}", name);
                    return name.to_string();
                }
            }
        }
        "4.0.0".into()
    }).clone()
}

/// 从 exe 文件读取版本信息（Windows 专用）
#[cfg(windows)]
fn read_exe_version(exe: &std::path::Path) -> Option<String> {
    // 简化实现：直接从目录名或文件名中提取版本号
    // ZCode 4.0.0 的目录结构通常是 D:\ProgramFiles\develop\ZCode\ZCode.exe
    // 或者便携版可能是 D:\ProgramFiles\develop\ZCode-4.0.0\ZCode.exe
    
    let filename = exe.file_name()?.to_str()?;
    
    // 如果文件名包含版本号模式 (X.Y.Z...)
    if filename.starts_with("ZCode") && filename.contains('.') {
        let parts: Vec<&str> = filename.split('.').collect();
        if parts.len() >= 3 && parts[1].chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("{}.{}", parts[1], parts[2]));
        }
    }
    
    // 尝试从父目录名提取
    if let Some(parent) = exe.parent()?.file_name()?.to_str() {
        if parent.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            let parts: Vec<&str> = parent.split('.').collect();
            if parts.len() >= 3 {
                return Some(format!("{}.{}", parts[0], parts[1]));
            }
        }
    }
    
    None
}

/// 从 exe 文件读取版本信息（非 Windows 备用实现）
#[cfg(not(windows))]
fn read_exe_version(_exe: &std::path::Path) -> Option<String> {
    // 非 Windows 平台暂不支持，返回 None
    None
}