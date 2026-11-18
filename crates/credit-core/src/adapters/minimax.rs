//! MiniMax Code 适配器（OAuth2 Device Code + PKCE）。
//! device/code、token、/minimax-cloud/api/v1/signin/{status,claim}、
//! /matrix/api/v1/commerce/get_membership_info、profile。
//! 恒定后缀 ?timezone_offset=28800&is_desktop=1&client=desktop。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::platform_fetch;
use reqwest::Method;

pub const DEVICE_CODE_URL: &str = "https://account.minimax.cn/oauth2/device/code";
pub const TOKEN_URL: &str = "https://account.minimax.cn/oauth2/token";
pub const AGENT_BASE: &str = "https://agent.minimax.cn";
pub const CLIENT_ID: &str = "mcode-public";
pub const SUFFIX: &str = "?timezone_offset=28800&is_desktop=1&client=desktop";

fn form(headers_token: Option<&str>, pairs: &[(&str, &str)]) -> (Vec<(String, String)>, Option<Value>) {
    let mut hs = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())];
    if let Some(t) = headers_token {
        hs.push(("Authorization".to_string(), format!("Bearer {t}")));
    }
    (hs, Some(json!(pairs.iter().map(|(k, v)| (*k, *v)).collect::<std::collections::HashMap<&str, &str>>())))
}

async fn post_form(url: &str, token: Option<&str>, pairs: &[(&str, &str)]) -> Result<Value, PlatformError> {
    // application/x-www-form-urlencoded 需要手写 body（reqwest .form 与 platform_fetch 的 json 冲突）
    let body = pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&");
    let mut hs: Vec<(String, String)> = vec![("Content-Type".into(), "application/x-www-form-urlencoded".into())];
    if let Some(t) = token {
        hs.push(("Authorization".into(), format!("Bearer {t}")));
    }
    let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    // post_form 直接用 platform_fetch 的 json 会错，改用原始 reqwest
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let mut req = client.post(url);
    for (k, v) in hs.iter() { req = req.header(*k, v); }
    let resp = req.body(body).send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() {
        401 => return Err(PlatformError::Auth),
        429 => return Err(PlatformError::Rate),
        403 => return Err(PlatformError::Forbidden),
        _ => {}
    }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    Ok(v)
}

fn chrono_now_plus(secs: u64) -> String {
    // RFC3339 UTC now + secs（std-only：用 SystemTime 手算日期）
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() + secs;
    let days = now / 86400;
    let rem = now % 86400;
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // civil_from_days (Howard Hinnant)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub struct MiniStart { pub device_code: String, pub verification_uri: String, pub interval_secs: u64, pub verifier: String }

pub async fn start_device_login() -> Result<MiniStart, PlatformError> {
    let mut vb = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut vb);
    let verifier = B64URL.encode(vb);
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    let v = post_form(DEVICE_CODE_URL, None, &[
        ("client_id", CLIENT_ID),
        ("scope", "agent.default"),
        ("audience", "agent-backend"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ]).await?;
    let uri = v.get("verification_uri_complete").or_else(|| v.get("verification_uri")).and_then(|u| u.as_str())
        .ok_or_else(|| PlatformError::Format("no verification_uri".into()))?;
    // origin 校验
    if !uri.starts_with("https://account.minimax.cn") {
        return Err(PlatformError::Network("verification uri origin mismatch".into()));
    }
    Ok(MiniStart {
        device_code: v.get("device_code").and_then(|d| d.as_str()).unwrap_or_default().to_string(),
        verification_uri: uri.to_string(),
        interval_secs: v.get("interval").and_then(|i| i.as_u64()).unwrap_or(3),
        verifier,
    })
}

pub enum MiniPoll { Pending { interval_secs: u64 }, Ready(Value, Value) }

pub async fn poll_device_login(st: &MiniStart) -> Result<MiniPoll, PlatformError> {
    let v = post_form(TOKEN_URL, None, &[
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", CLIENT_ID),
        ("device_code", st.device_code.as_str()),
        ("code_verifier", st.verifier.as_str()),
    ]).await;
    match v {
        Err(PlatformError::Format(_)) | Err(PlatformError::Business { .. }) => {
            // 400 分支在 post_form 里表现为非 JSON 或业务错误；轮询端点 400 带 error 字段
            return Ok(MiniPoll::Pending { interval_secs: st.interval_secs });
        }
        Err(e) => return Err(e),
        Ok(v) => {
            if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                let mut interval = st.interval_secs;
                if err == "slow_down" { interval += 5; }
                return Ok(MiniPoll::Pending { interval_secs: interval });
            }
            let at = v.get("access_token").and_then(|t| t.as_str()).ok_or_else(|| PlatformError::Format("no access_token".into()))?;
            Ok(MiniPoll::Ready(Value::String(at.to_string()), v.clone()))
        }
    }
}

async fn api_get(c: &Cred, path: &str) -> Result<Value, PlatformError> {
    let hs = vec![
        ("Authorization".to_string(), format!("Bearer {}", c.token)),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&format!("{AGENT_BASE}{path}"), Method::GET, None, &hs).await
}

async fn api_post(c: &Cred, path: &str, body: Option<Value>) -> Result<Value, PlatformError> {
    let hs = vec![
        ("Authorization".to_string(), format!("Bearer {}", c.token)),
        ("Content-Type".to_string(), "application/json".to_string()),
    ];
    let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&format!("{AGENT_BASE}{path}"), Method::POST, body, &hs).await
}

pub struct MiniMaxAdapter;

#[async_trait::async_trait]
impl Adapter for MiniMaxAdapter {
    fn provider(&self) -> &'static str { "minimax" }

    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let v = api_get(c, "/matrix/api/v1/user/profile").await?;
        let d = v;
        let status = d.get("base_resp").and_then(|b| b.get("status_code")).and_then(|s| s.as_i64());
        if status != Some(0) {
            return Err(PlatformError::business_of(&d));
        }
        Ok(d.get("data").cloned().unwrap_or(d.clone()))
    }

    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let v = api_get(c, &format!("/minimax-cloud/api/v1/signin/status{SUFFIX}")).await?;
        let status = v.get("base_resp").and_then(|b| b.get("status_code")).and_then(|s| s.as_i64());
        if status != Some(0) { return Err(PlatformError::business_of(&v)); }
        let days = v.pointer("/data/days").and_then(|d| d.as_array()).cloned().unwrap_or_default();
        let today = days.iter().find(|d| d.get("is_today").and_then(|t| t.as_bool()).unwrap_or(false)).cloned().unwrap_or(Value::Null);
        let st = today.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
        // 2=可领 3=已签
        Ok(CheckinState { checked: st == 3, claimable: st == 2, raw: today, ..Default::default() })
    }

    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: Some("今日已领取".into()) });
        }
        if !st.claimable {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: None });
        }
        let v = api_post(c, &format!("/minimax-cloud/api/v1/signin/claim{SUFFIX}"), None).await?;
        let status = v.get("base_resp").and_then(|b| b.get("status_code")).and_then(|s| s.as_i64());
        if status != Some(0) { return Err(PlatformError::business_of(&v)); }
        let r = v.get("data").and_then(|d| d.get("claim_result")).and_then(|r| r.as_i64()).unwrap_or(0);
        Ok(match r {
            1 => Receipt { status: "success".into(), amount: v.pointer("/data/points").and_then(|p| p.as_f64()), message: None },
            2 => Receipt { status: "already".into(), amount: None, message: Some("今日已领过".into()) },
            _ => Receipt { status: "pending".into(), amount: None, message: Some(format!("claim_result={r}")) },
        })
    }

    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let v = api_post(c, "/matrix/api/v1/commerce/get_membership_info", Some(json!({}))).await?;
        let status = v.get("base_resp").and_then(|b| b.get("status_code")).and_then(|s| s.as_i64());
        if status != Some(0) { return Err(PlatformError::business_of(&v)); }
        let remaining = v.pointer("/op_credit_summary/total_remaining_amount").and_then(|r| r.as_f64())
            .or_else(|| v.get("total_remains_credit").and_then(|r| r.as_f64()));
        Ok(Quota { remaining, parts: Value::Null, sampled_at: now_iso() })
    }

    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let Some(rt) = &c.refresh_token else { return Err(PlatformError::Auth) };
        let v = post_form(TOKEN_URL, None, &[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt.as_str()),
            ("client_id", CLIENT_ID),
            ("scope", "agent.default"),
            ("audience", "agent-backend"),
        ]).await?;
        // OAuth 端点 400 也返回 JSON（invalid_grant 等）—— 必须显式检查 error 字段
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return Err(PlatformError::Business {
                code: serde_json::json!(err),
                message: if err == "invalid_grant" {
                    "MiniMax 授权已失效（refresh_token 已轮换作废），请重新登录 MiniMax Code 账号".into()
                } else {
                    format!("MiniMax refresh 失败: {err}")
                },
            });
        }
        let Some(at) = v.get("access_token").and_then(|t| t.as_str()) else {
            return Err(PlatformError::Auth);
        };
        let mut out = c.clone();
        out.token = at.to_string();
        // refresh_token 轮换：新 RT 必须保存，否则旧 RT 立即作废
        if let Some(r) = v.get("refresh_token").and_then(|t| t.as_str()) { out.refresh_token = Some(r.to_string()); }
        out.expires_at = v.get("expires_in").and_then(|e| e.as_u64())
            .map(|secs| (chrono_now_plus(secs)));
        Ok(out)
    }
}
