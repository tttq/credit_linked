//! MiniMax Code 本机凭据扫描（新版 CLI auth.json 明文格式）。
//! 路径: ~/.minimax/auth/prod/{cn|intl}/mcode-public/auth.json

use serde_json::Value;
use crate::adapters::Cred;

pub struct MiniMaxLiveAuth { pub token: String, pub refresh_token: Option<String>, pub expires_at: Option<String>, pub region: String }

fn auth_dir() -> std::path::PathBuf {
    let home = std::env::var("MINIMAX_HOME").ok().map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir()).unwrap_or_default();
    home.join(".minimax").join("auth").join("prod")
}

/// 读当前登录的 MiniMax Code 凭据（auth.json records 中第一条有效记录）
pub fn read_live_auth() -> Option<MiniMaxLiveAuth> {
    let root = auth_dir();
    for region in ["cn", "intl", "global"] {
        let dir = root.join(region).join("mcode-public");
        let file = dir.join("auth.json");
        if !file.exists() { continue; }
        let v: Value = serde_json::from_reader(std::fs::File::open(&file).ok()?).ok()?;
        let records = v.get("records")?.as_object()?;
        for (_key, rec) in records {
            let token = rec.get("accessToken")?.as_str()?.to_string();
            if token.is_empty() { continue; }
            let refresh = rec.get("refreshToken").and_then(|r| r.as_str()).map(|s| s.to_string());
            let expires_ms = rec.get("expiresAtMs").and_then(|e| e.as_u64()).unwrap_or(0);
            let expires = if expires_ms > 0 {
                // 简单 ISO（UTC）
                let secs = expires_ms / 1000;
                let days = (secs / 86400) as i64;
                let rem = secs % 86400;
                let (y, mo, d) = crate::adapters::civil_from_days(days);
                Some(format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, rem / 3600, (rem % 3600) / 60, rem % 60))
            } else { None };
            return Some(MiniMaxLiveAuth { token, refresh_token: refresh, expires_at: expires, region: region.to_string() });
        }
    }
    None
}

/// 直接转换为入库凭据
pub fn to_cred(auth: MiniMaxLiveAuth) -> Cred {
    Cred {
        provider: "minimax".into(),
        uid: String::new(),
        name: "MiniMax Code".into(),
        token: auth.token,
        refresh_token: auth.refresh_token,
        expires_at: auth.expires_at,
        meta: serde_json::json!({ "region": auth.region }),
    }
}