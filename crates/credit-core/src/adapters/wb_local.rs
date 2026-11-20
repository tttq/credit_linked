//! WorkBuddy 本机会话读取（vendor/workbuddyLocal.mjs 对齐）。
//! 会话文件 %LOCALAPPDATA%/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info
//! （intl: workbuddy-desktop-ai.info）。$wbEncrypted 信封 AES-256-GCM，
//! 主密钥 = SHA256(atRestSecretKey)（spawn 官方客户端 ELECTRON_RUN_AS_NODE 取出）。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::crypto::aes_gcm::gcm_decrypt;

/// 会话目录：各平台 CodeBuddyExtension/Data/Public/auth
pub fn auth_dir() -> std::path::PathBuf {
    // Windows: %LOCALAPPDATA%/CodeBuddyExtension/Data/Public/auth
    // macOS:   ~/Library/Application Support/CodeBuddyExtension/Data/Public/auth
    // Linux:   ~/.config/CodeBuddyExtension/Data/Public/auth
    let base = crate::platform_dirs::electron_local_root();
    base.join("CodeBuddyExtension").join("Data").join("Public").join("auth")
}

/// 会话文件路径（intl 版后缀 -ai）
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
    // 字段级信封（framing=field）解出的是裸字符串（如 accessToken JWT），不是 JSON；
    // 文件级信封解出的是 JSON。先尝试 JSON，失败则按 UTF-8 字符串处理。
    if let Ok(v) = serde_json::from_slice::<Value>(&pt) {
        return Some(v);
    }
    String::from_utf8(pt).ok().map(Value::String)
}

/// spawn 官方客户端取 atRestSecretKey（与原版逐字同脚本）。
/// client_exe 定位交给上层；这里只负责 spawn+哈希（跨平台 ELECTRON_RUN_AS_NODE）。
pub fn master_key_from_client(client_exe: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    const SCRIPT: &str = "const b=process._linkedBinding('electron_browser_workbuddy_storage');const p=JSON.parse(b.loggerGet());process.stdout.write(p.atRestSecretKey)";
    let out = crate::proc_util::hide_console(std::process::Command::new(client_exe))
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

/// 本机 WorkBuddy 客户端 exe 候选（解密会话需要启动它取 atRestSecretKey）
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        format!("{local}/Programs/WorkBuddy/WorkBuddy.exe"),
        "D:/ProgramFiles/software/WorkBuddy/WorkBuddy.exe".to_string(),
        "C:/Program Files/WorkBuddy/WorkBuddy.exe".to_string(),
        "D:/ProgramFiles/develop/WorkBuddy/WorkBuddy.exe".to_string(),
        format!("{local}/Programs/CodeBuddy/CodeBuddy.exe"),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机 WorkBuddy/CodeBuddy 客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 读取本机 WorkBuddy 客户端真实版本（`resources/install-manifest.json` 的 appVersion）。
///
/// 聊天头里的 `X-IDE-Version` 与 UA 版本段都必须是**当前安装版本**：
/// 写死版本在客户端升级后会立刻变成「版本对不上」的显式特征。
pub fn app_version() -> Option<String> {
    // 优先从 exe 位置推导 resources/install-manifest.json
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(exe) = find_client_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources/install-manifest.json"));
            candidates.push(dir.join("install-manifest.json"));
        }
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    for base in [
        format!("{local}/Programs/WorkBuddy"),
        format!("{local}/Programs/CodeBuddy"),
        "D:/ProgramFiles/software/WorkBuddy".to_string(),
        "C:/Program Files/WorkBuddy".to_string(),
    ] {
        candidates.push(std::path::PathBuf::from(&base).join("resources/install-manifest.json"));
    }
    for p in candidates {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s.trim_start_matches('\u{feff}')) {
                if let Some(ver) = v.get("appVersion").and_then(|x| x.as_str()) {
                    let ver = ver.trim().to_string();
                    if !ver.is_empty() {
                        return Some(ver);
                    }
                }
            }
        }
    }
    // 环境变量兜底（客户端目录结构变化时用户可手动指定）
    std::env::var("WORKBUDDY_APP_VERSION").ok().filter(|s| !s.trim().is_empty())
}

/// 会话文件原始字节指纹（客户端刷新 token 会重写文件；不解密，轮询零开销）
pub fn session_fingerprint(intl: bool) -> Option<String> {
    let bytes = std::fs::read(session_file(intl)).ok()?;
    Some(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string())
}

/// 启动本机 WorkBuddy 客户端（已运行则单实例应用聚焦现有窗口）。
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

/// 等待客户端刷新会话（文件指纹变化），最多 timeout。返回 Some = 已刷新。
pub fn wait_for_session_change(intl: bool, old_fp: Option<&str>, timeout: std::time::Duration) -> Option<Value> {
    let start = std::time::Instant::now();
    loop {
        let cur_fp = session_fingerprint(intl);
        let changed = match (old_fp, cur_fp.as_deref()) {
            (Some(old), Some(cur)) => cur != old,
            (None, Some(_)) => true,
            _ => false,
        };
        if changed {
            return read_session_full(intl).ok().flatten();
        }
        if start.elapsed() >= timeout { return None; }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

fn value_has_envelope(v: &Value) -> bool {
    if is_wb_encrypted(v) {
        return true;
    }
    match v {
        Value::Object(map) => map.values().any(value_has_envelope),
        Value::Array(arr) => arr.iter().any(value_has_envelope),
        _ => false,
    }
}

/// 递归解密字段级信封（客户端新版把 auth.accessToken 等字段单独包 $wbEncrypted）
fn decrypt_tree(v: &mut Value, key: &[u8]) {
    match v {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                let child = map.get(&k).cloned().unwrap_or(Value::Null);
                if is_wb_encrypted(&child) {
                    if let Some(plain) = open_envelope(&child, key) {
                        map.insert(k, plain);
                        continue;
                    }
                }
                if let Some(slot) = map.get_mut(&k) {
                    decrypt_tree(slot, key);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                decrypt_tree(item, key);
            }
        }
        _ => {}
    }
}

/// 读会话并按需解密字段级信封（新版 WorkBuddy 逐字段加密）。
/// 返回错误说明为什么无法解密（客户端未安装/启动失败），便于 UI 提示。
pub fn read_session_full(intl: bool) -> anyhow::Result<Option<Value>> {
    let raw = match read_session(intl, None) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        // 整文件加密（旧格式）：需要有客户端 exe 才能解
        Err(_) => {
            let exe = find_client_exe().ok_or_else(|| anyhow::anyhow!("未找到 WorkBuddy 客户端，无法读取加密会话"))?;
            match read_session(intl, Some(&exe))? {
                Some(v) => v,
                None => return Ok(None),
            }
        }
    };
    if !value_has_envelope(&raw) {
        return Ok(Some(raw));
    }
    let exe = find_client_exe()
        .ok_or_else(|| anyhow::anyhow!("会话字段已加密，但本机未找到 WorkBuddy 客户端（无法取得解密密钥）"))?;
    let key = master_key_from_client(&exe).map_err(|e| anyhow::anyhow!("启动 WorkBuddy 取解密密钥失败: {e}"))?;
    let mut session = raw;
    decrypt_tree(&mut session, &key);
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
