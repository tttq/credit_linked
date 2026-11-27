//! MiniMax Code 本机凭据扫描（新版 CLI auth.json 明文格式）。
//! 路径：~/.minimax/auth/prod/{cn|intl}/mcode-public/auth.json

use crate::debug_log;
use crate::adapters::Cred;
use base64::Engine;
use serde_json::Value;

/// 本机 MiniMax Code 登录态（auth.json records 中一条有效记录）
#[derive(Clone)]
pub struct MiniMaxLiveAuth {
    pub token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    /// accessToken 原始到期时间（epoch ms，客户端文件原值）
    pub expires_ms: Option<u64>,
    pub region: String,
    /// auth.json records 的键后缀（`com.minimax.mcode.oauth.prod.cn <键>` 的第二段）。
    /// 这是客户端为「账号」生成的稳定标识：accessToken 轮换时不变，可当 uid 兜底。
    pub account_key: Option<String>,
}

/// records 键 → 账号标识。实测键是 `com.minimax.mcode.oauth.prod.cn\0<账号键>`
/// （分隔符是 **NUL 字节**，不是空格，所以必须先归一化控制字符再切分）。
/// 不带分隔符（老版本只有 clientId）时返回 None。
fn account_key_of(record_key: &str) -> Option<String> {
    let cleaned: String = record_key
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let key = cleaned.split_whitespace().last().unwrap_or("");
    if key.is_empty() || key == cleaned.trim() || key.len() < 8 {
        return None;
    }
    Some(key.to_string())
}

/// 无账号标识时的兜底 uid：对「稳定种子」（refreshToken → accessToken）取短哈希。
/// 目的只有一个——**绝不写空 uid**：空 uid 会被 ui_snapshot 的脏行清理删掉，
/// 表现出来就是「提示导入成功但账号列表里没有」。
fn fallback_uid(auth: &MiniMaxLiveAuth) -> String {
    use sha2::{Digest, Sha256};
    let seed = auth.refresh_token.as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(auth.token.as_str());
    let hex: String = Sha256::digest(seed.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("mm-{}", &hex[..16])
}

fn auth_dir() -> std::path::PathBuf {
    let home = std::env::var("MINIMAX_HOME").ok().map(std::path::PathBuf::from)
        .or_else(dirs::home_dir).unwrap_or_default();
    
    // Try multiple possible paths (app data vs home dir)
    let candidates = vec![
        // AppData roaming (most common for Electron apps)
        std::env::var("APPDATA").ok().map(|p| std::path::PathBuf::from(p).join("MiniMax")).filter(|p| p.exists()),
        // Home directory variants
        Some(home.join(".minimax")),
        Some(home.join(".mavis")), // Alternative name used by some versions
    ].into_iter().flatten();
    
    for base in candidates {
        let auth_dir = base.join("auth").join("prod");
        if auth_dir.exists() {
            return auth_dir;
        }
    }
    
    // Fallback to default
    home.join(".minimax").join("auth").join("prod")
}

/// 读本机全部 MiniMax Code 登录态（auth.json records 里每条有效记录一条；
/// 客户端支持多账号时 records 会有多个键）。
pub fn read_all_live_auths() -> Vec<MiniMaxLiveAuth> {
    let root = auth_dir();

    debug_log::debug_log("MiniMax", &format!("Checking auth directory: {:?}", root));

    let mut out: Vec<MiniMaxLiveAuth> = Vec::new();
    for region in ["cn", "intl", "global"] {
        let dir = root.join(region).join("mcode-public");
        let file = dir.join("auth.json");

        debug_log::debug_log("MiniMax", &format!("Looking for auth file: {:?}", file));

        if !file.exists() {
            continue;
        }

        match std::fs::File::open(&file) {
            Ok(f) => {
                if let Ok(v) = serde_json::from_reader::<_, serde_json::Value>(f) {
                    let schema = v.get("schemaVersion").and_then(|x| x.as_i64()).unwrap_or(-1);
                    debug_log::debug_log("MiniMax", &format!("Parsed auth.json successfully, schemaVersion: {}", schema));

                    if let Some(records) = v.get("records").and_then(|x| x.as_object()) {
                        debug_log::debug_log("MiniMax", &format!("Found {} records in {}", records.len(), file.display()));

                        for (key, rec) in records {
                            debug_log::debug_log("MiniMax", &format!("Processing record key: '{}'", key));

                            // 单条记录缺字段只跳过它，不能短路整次扫描（旧实现用 `?`，
                            // 第一条记录不合法就直接返回 None → 后面的账号全部读不到）。
                            let Some(token) = rec.get("accessToken").and_then(|t| t.as_str()) else {
                                debug_log::debug_log("MiniMax", &format!("Record '{}' has no accessToken, skipping", key));
                                continue;
                            };
                            let token = token.trim().to_string();
                            if token.is_empty() {
                                debug_log::debug_log("MiniMax", &format!("Record '{}' has empty accessToken, skipping", key));
                                continue;
                            }

                            debug_log::debug_log("MiniMax", &format!("Token length: {}", token.len()));

                            let refresh = rec.get("refreshToken").and_then(|r| r.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty());
                            let expires_ms = rec.get("expiresAtMs").and_then(|e| e.as_u64()).filter(|ms| *ms > 0);
                            let expires = expires_ms.map(|ms| {
                                let secs = ms / 1000;
                                let days = (secs / 86400) as i64;
                                let rem = secs % 86400;
                                let (y, mo, d) = crate::adapters::civil_from_days(days);
                                format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, rem / 3600, (rem % 3600) / 60, rem % 60)
                            });

                            let account_key = account_key_of(key);
                            debug_log::debug_log("MiniMax", &format!(
                                "Read auth for region {} (accountKey: {})",
                                region, account_key.clone().unwrap_or_else(|| "<none>".into())
                            ));

                            // 同一 accessToken 可能同时出现在 .minimax 与 .mavis 两处（互为镜像）
                            if out.iter().any(|a| a.token == token) {
                                continue;
                            }
                            out.push(MiniMaxLiveAuth {
                                token,
                                refresh_token: refresh,
                                expires_at: expires,
                                expires_ms,
                                region: region.to_string(),
                                account_key,
                            });
                        }
                    } else {
                        debug_log::error_log("MiniMax", "'records' field is not an object");
                        debug_log::debug_log("MiniMax", "TIP: Check the structure of auth.json");
                    }
                }
            }
            Err(e) => {
                debug_log::error_log("MiniMax", &format!("Error reading auth file {:?}: {}", file, e));
            }
        }
    }

    if out.is_empty() {
        debug_log::debug_log("MiniMax", "No valid authentication found");
    }
    out
}

/// 读当前登录的 MiniMax Code 凭据（auth.json records 中第一条有效记录）
pub fn read_live_auth() -> Option<MiniMaxLiveAuth> {
    read_all_live_auths().into_iter().next()
}

/// 从候选登录态中精确匹配一个已有账号。
///
/// 匹配优先级：稳定 accountKey → access token → refresh token。
/// 故意不提供“只有一个候选就返回”的兜底：浏览器登录账号与本机客户端账号可能不同，
/// 多账号场景下这种兜底会把 A 的 token 写进 B。
pub fn select_matching_live_auth<'a>(all: &'a [MiniMaxLiveAuth], cred: &Cred) -> Option<&'a MiniMaxLiveAuth> {
    let account_key = cred.meta.get("accountKey").and_then(Value::as_str)
        .map(str::trim).filter(|s| !s.is_empty());
    if let Some(key) = account_key {
        if let Some(auth) = all.iter().find(|a| a.account_key.as_deref().map(str::trim) == Some(key)) {
            return Some(auth);
        }
    }
    if !cred.token.is_empty() {
        if let Some(auth) = all.iter().find(|a| a.token == cred.token) {
            return Some(auth);
        }
    }
    if let Some(rt) = cred.refresh_token.as_deref().filter(|s| !s.is_empty()) {
        if let Some(auth) = all.iter().find(|a| a.refresh_token.as_deref() == Some(rt)) {
            return Some(auth);
        }
    }
    None
}

/// 读取与指定账号匹配的本机登录态；没有可靠匹配时返回 None。
pub fn find_matching_live_auth(cred: &Cred) -> Option<MiniMaxLiveAuth> {
    select_matching_live_auth(&read_all_live_auths(), cred).cloned()
}

/// 直接转换为入库凭据。
///
/// uid 解析顺序（**绝不允许为空**）：
/// ① accessToken 是 JWT → 取 payload 里的 user_id/uid/sub…；
/// ② 不是 JWT（本机实测签发的是 `mmoat_*` 不透明串）→ 取 auth.json records 键后缀（账号稳定标识）；
/// ③ 都拿不到 → refreshToken/accessToken 短哈希。
/// 空 uid 会被 `ui_snapshot()` 里的脏行清理删掉，UI 表现为「导入成功但列表没有」。
pub fn to_cred(auth: MiniMaxLiveAuth) -> Cred {
    let local_fingerprint = auth_fingerprint(&auth);
    let mut uid = extract_uid_from_token(&auth.token);
    if uid.trim().is_empty() {
        if let Some(k) = auth.account_key.as_deref() {
            if !k.trim().is_empty() {
                uid = k.trim().to_string();
            }
        }
    }
    if uid.trim().is_empty() {
        uid = fallback_uid(&auth);
    }

    Cred {
        provider: "minimax".into(),
        uid,
        name: "MiniMax Code".into(),
        token: auth.token,
        refresh_token: auth.refresh_token,
        expires_at: auth.expires_at,
        meta: serde_json::json!({
            "region": auth.region,
            "accountKey": auth.account_key,
            "expiresAtMs": auth.expires_ms,
            "localFingerprint": local_fingerprint,
        }),
    }
}

/// 从 token 中提取 uid（如果 token 是 JWT 格式）
fn extract_uid_from_token(token: &str) -> String {
    if token.len() < 50 {
        return String::new();
    }
    
    // 尝试解码 JWT token
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return String::new();
    }
    
    // 解码 payload 部分
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1].as_bytes()) {
        if let Ok(json_str) = std::str::from_utf8(&decoded) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                // 尝试常见的 uid 字段名
                for field in &["user_id", "uid", "sub", "userId", "id"] {
                    if let Some(val) = json.get(*field) {
                        if let Some(s) = val.as_str() {
                            if !s.is_empty() {
                                return s.to_string();
                            }
                        }
                    }
                }
            }
        }
    }
    
    String::new()
}

/// 登录态指纹（access token + refresh token，不泄露明文）。
fn auth_fingerprint(auth: &MiniMaxLiveAuth) -> Option<String> {
    use sha2::{Digest, Sha256};
    if auth.token.is_empty() { return None; }
    let seed = format!("{}\0{}", auth.token, auth.refresh_token.as_deref().unwrap_or(""));
    Some(Sha256::digest(seed.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string())
}

/// token 指纹（识别 MiniMax Code 客户端是否已刷新登录态；不泄露 token 明文）
pub fn token_fingerprint() -> Option<String> {
    read_live_auth().and_then(|a| auth_fingerprint(&a))
}

/// 指定账号的本机登录态指纹（多账号场景不能用第一条记录代替）。
pub fn token_fingerprint_for(cred: &Cred) -> Option<String> {
    find_matching_live_auth(cred).and_then(|a| auth_fingerprint(&a))
}

/// 本机 MiniMax Code 客户端 exe 候选
pub fn client_exe_candidates() -> Vec<std::path::PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    [
        format!("{local}/Programs/minimax-code/MiniMax Code.exe"),
        format!("{local}/Programs/MiniMax Code/MiniMax Code.exe"),
        "D:/ProgramFiles/develop/MiniMax Code/MiniMax Code.exe".to_string(),
        format!("{local}/minimax-code/MiniMax Code.exe"),
    ].into_iter().map(std::path::PathBuf::from).collect()
}

/// 找本机 MiniMax Code 客户端 exe（已安装才返回）
pub fn find_client_exe() -> Option<std::path::PathBuf> {
    client_exe_candidates().into_iter().find(|p| p.exists())
}

/// 启动本机 MiniMax Code 客户端（已运行则单实例应用聚焦现有窗口）。
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

/// 等待指定账号的本机登录态变化（access token 或 refresh token），最多 timeout。
pub fn wait_for_matching_token_change(cred: &Cred, timeout: std::time::Duration) -> Option<MiniMaxLiveAuth> {
    let start = std::time::Instant::now();
    let old_fp = token_fingerprint_for(cred);
    loop {
        if let Some(live) = find_matching_live_auth(cred) {
            if auth_fingerprint(&live).as_deref() != old_fp.as_deref() {
                return Some(live);
            }
        }
        if start.elapsed() >= timeout { return None; }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

/// 等待客户端刷新 token（指纹变化），最多 timeout。返回 Some = 已刷新。
pub fn wait_for_token_change(old_fp: Option<&str>, timeout: std::time::Duration) -> Option<MiniMaxLiveAuth> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：records 键的分隔符是 NUL（`clientId\0账号键`），必须能解析出账号键。
    #[test]
    fn record_key_splits_on_nul() {
        let k = "com.minimax.mcode.oauth.prod.cn\u{0}L5v6Na6pPThgjT-L-XpF5lOVe1YvYZs_eM_0P5wd1Ec";
        assert_eq!(account_key_of(k).as_deref(), Some("L5v6Na6pPThgjT-L-XpF5lOVe1YvYZs_eM_0P5wd1Ec"));
        // 老格式（只有 clientId）不产生 uid
        assert_eq!(account_key_of("com.minimax.mcode.oauth.prod.cn"), None);
    }

    /// 回归：accessToken 是 `mmoat_*` 不透明串（非 JWT）时也必须产出稳定非空 uid，
    /// 否则会被 ui_snapshot 的脏行清理删除（导入成功但列表没有）。
    #[test]
    fn to_cred_uid_never_empty() {
        let live = MiniMaxLiveAuth {
            token: "mmoat_tKaPbKNrLQ13juIdwCXJYETWV-YvODmqdojgck_2Mm4dfM3WjUAh-g".into(),
            refresh_token: Some("mmort_gs1ymNw5LTucsTHTKkxPBrv-0yCCcKA".into()),
            expires_at: Some("2026-11-24T00:56:25Z".into()),
            expires_ms: Some(1795481785976),
            region: "cn".into(),
            account_key: Some("L5v6Na6pPThgjT-L-XpF5lOVe1YvYZs_eM_0P5wd1Ec".into()),
        };
        let c = to_cred(live.clone());
        assert_eq!(c.uid, "L5v6Na6pPThgjT-L-XpF5lOVe1YvYZs_eM_0P5wd1Ec");
        assert_eq!(c.provider, "minimax");
        assert_eq!(c.meta["region"], "cn");
        assert!(c.meta["localFingerprint"].is_string());

        // 无记录键（老客户端）→ 哈希兜底，仍然非空且稳定
        let mut no_key = live.clone();
        no_key.account_key = None;
        let c2 = to_cred(no_key.clone());
        assert!(!c2.uid.trim().is_empty());
        assert_eq!(c2.uid, to_cred(no_key).uid);
    }

    fn live(key: &str, token: &str, refresh: &str) -> MiniMaxLiveAuth {
        MiniMaxLiveAuth {
            token: token.into(),
            refresh_token: Some(refresh.into()),
            expires_at: Some("2026-11-28T00:00:00Z".into()),
            expires_ms: Some(1_900_000_000_000),
            region: "cn".into(),
            account_key: Some(key.into()),
        }
    }

    #[test]
    fn matching_live_auth_never_crosses_accounts() {
        let all = vec![
            live("account-a", "token-a", "refresh-a"),
            live("account-b", "token-b", "refresh-b"),
        ];
        let mut a = to_cred(all[0].clone());
        a.uid = "profile-a".into();
        a.meta["accountKey"] = serde_json::json!("account-a");
        assert_eq!(select_matching_live_auth(&all, &a).unwrap().token, "token-a");

        let mut legacy_b = to_cred(all[1].clone());
        legacy_b.meta = serde_json::json!({});
        legacy_b.token = "token-b".into();
        assert_eq!(select_matching_live_auth(&all, &legacy_b).unwrap().token, "token-b");

        let unknown = Cred { token: "unknown".into(), refresh_token: Some("unknown-r".into()), ..Default::default() };
        assert!(select_matching_live_auth(&all, &unknown).is_none());
    }
}
