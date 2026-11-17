//! 华为 CodeArts 适配器（Agent/Desktop）。
//! STS token（DPoP ES256 p1363）+ SDK-HMAC-SHA256 请求签名 + /v1/ops/{delivery,claim,confirm}。
//! 幂等键 SHA256("{uid}:{day}:{campaignId}")。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use p256::{SecretKey, ecdsa::signature::Signer, ecdsa::SigningKey};

use crate::adapters::*;
use crate::adapters::trae::urlencoding_lite;
use crate::error::PlatformError;
use crate::net::http::platform_fetch;
use reqwest::Method;

pub const STS_BASE: &str = "https://sts.cn-north-4.myhuaweicloud.com";
pub const API_BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";

pub struct CodeArtsAdapter { pub desktop: bool }

// ===== DPoP =====

pub fn dpop_header(private_jwk: &Value, htu: &str) -> Result<String, PlatformError> {
    let sk = SigningKey::from(JwkDec::from_jwk(private_jwk)?);
    let header = B64URL.encode(json!({
        "typ": "dpop+jwt", "alg": "ES256", "jwk": public_jwk_of(&sk),
    }).to_string());
    let payload = B64URL.encode(json!({
        "htu": htu, "htm": "POST", "iat": now_secs(), "jti": uuid::Uuid::new_v4().to_string(),
    }).to_string());
    let msg = format!("{header}.{payload}");
    let sig: p256::ecdsa::Signature = sk.sign(msg.as_bytes()); // p1363: r||s
    Ok(format!("{msg}.{}", B64URL.encode(sig.to_bytes())))
}

struct JwkDec;
impl JwkDec {
    fn from_jwk(jwk: &Value) -> Result<SecretKey, PlatformError> {
        let d = jwk.get("d").and_then(|x| x.as_str()).ok_or_else(|| PlatformError::Format("jwk missing d".into()))?;
        let d_bytes = b64url_decode(d)?;
        let sk = SecretKey::from_slice(&d_bytes).map_err(|e| PlatformError::Format(format!("bad jwk d: {e}")))?;
        Ok(sk)
    }
}

pub fn public_jwk_of(sk: &SigningKey) -> Value {
    let vk = sk.verifying_key();
    let enc = vk.to_encoded_point(false); // uncompressed: 04||X||Y
    let bytes = enc.as_bytes();
    let x = b64url_encode(&bytes[1..33]);
    let y = b64url_encode(&bytes[33..65]);
    json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })
}

pub fn b64url_decode(s: &str) -> Result<Vec<u8>, PlatformError> {
    B64URL.decode(s).map_err(|e| PlatformError::Format(format!("b64url: {e}")))
}
pub fn b64url_encode(b: &[u8]) -> String { B64URL.encode(b) }

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

// ===== STS token（AK/SK/securityToken） =====

/// POST /v1/oauth2/tokens（grant_type=authorization_code 或 refresh_token）+ DPoP
pub async fn sts_token(body_pairs: &[(&str, &str)], private_jwk: &Value) -> Result<Value, PlatformError> {
    let url = format!("{STS_BASE}/v1/oauth2/tokens");
    let dpop = dpop_header(private_jwk, &url)?;
    let body = body_pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&");
    let _hs = vec![
        ("Content-Type", "application/x-www-form-urlencoded".to_string()),
        ("DPoP", dpop.clone()),
    ];
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let resp = client.post(&url).header("Content-Type", "application/x-www-form-urlencoded").header("DPoP", dpop).body(body)
        .send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() { 401 => return Err(PlatformError::Auth), 429 => return Err(PlatformError::Rate), _ => {} }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    Ok(v)
}

// ===== SDK-HMAC-SHA256 请求签名 =====

pub fn decode_percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 && i + 2 <= b.len() - 1 + 1 && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16).unwrap_or(99) as u32;
            let l = (b[i + 2] as char).to_digit(16).unwrap_or(99) as u32;
            if h < 16 && l < 16 { out.push(((h << 4) | l) as u8); i += 3; continue; }
        }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sign_codearts(
    ak: &str, sk: &str, method: &str, host: &str, url_path: &str, query: &str,
    headers: &[(String, String)], body: &str, security_token: Option<&str>,
) -> (String, Vec<(String, String)>) {
    // 参与签名的 header：content-type / host / x-sdk-date [+ x-security-token] [+ 业务头]
    let mut sign_headers: Vec<(String, String)> = headers.to_vec();
    let x_sdk_date = gmt_iso8601();
    sign_headers.push(("host".into(), host.to_string()));
    if !sign_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-sdk-date")) {
        sign_headers.push(("x-sdk-date".into(), x_sdk_date.clone()));
    }
    if let Some(st) = security_token {
        sign_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("x-security-token"));
        sign_headers.push(("x-security-token".into(), st.to_string()));
    }
    // canonical: 小写化 header 名并排序
    sign_headers.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let canonical_headers: String = sign_headers.iter()
        .map(|(k, v)| format!("{}:{}\n", k.to_lowercase(), v.trim()))
        .collect();
    let signed_header_names: String = sign_headers.iter().map(|(k, _)| k.to_lowercase()).collect::<Vec<_>>().join(";");
    let body_hash = hex_lower(&Sha256::digest(body.as_bytes()));
    let canonical_query = canonical_query_str(query);
    let mut uri = url_path.split('/').map(|seg| {
        // vendor: encode(decodeURIComponent(seg)) per segment
        urlencoding_lite(&decode_percent(seg))
    }).collect::<Vec<_>>().join("/");
    if !uri.ends_with('/') { uri.push('/'); }
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(), uri, canonical_query, canonical_headers, signed_header_names, body_hash
    );
    let hashed = hex_lower(&Sha256::digest(canonical.as_bytes()));
    let mut mac = Hmac::<Sha256>::new_from_slice(sk.as_bytes()).unwrap();
    mac.update(format!("SDK-HMAC-SHA256\n{x_sdk_date}\n{hashed}").as_bytes());
    let sig = hex_lower(&mac.finalize().into_bytes());
    let auth = format!("SDK-HMAC-SHA256 Access={ak}, SignedHeaders={signed_header_names}, Signature={sig}");
    (auth, sign_headers)
}

fn canonical_query_str(query: &str) -> String {
    // 排序 + 编码（原版使用严格 URL 编码规则；对常见 query 对齐）
    let mut pairs: Vec<(String, String)> = query.split('&').filter(|s| !s.is_empty()).map(|p| {
        match p.split_once('=') { Some((k, v)) => (k.to_string(), v.to_string()), None => (p.to_string(), String::new()) }
    }).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&")
}

fn gmt_iso8601() -> String {
    // yyyymmddTHHMMSSZ（压缩 ISO8601，UTC）
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, mo, d) = crate::adapters::civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ===== 业务请求 =====

fn channel(desktop: bool) -> &'static str { if desktop { "DESKTOP" } else { "IDE" } }

async fn ops_request(c: &Cred, desktop: bool, method: Method, path: &str, query: &str, body: Value, identity: bool) -> Result<Value, PlatformError> {
    let base = if identity { STS_BASE } else { API_BASE };
    let url = format!("{base}{path}");
    let ak = c.meta.get("access_key_id").and_then(|a| a.as_str()).unwrap_or_default();
    let sk = c.meta.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or_default();
    let st = c.meta.get("security_token").and_then(|a| a.as_str());
    let body_str = if body.is_null() { String::new() } else { serde_json::to_string(&body).unwrap_or_default() };
    let mut base_headers: Vec<(String, String)> = vec![("content-type".into(), "application/json".into())];
    if desktop {
        base_headers.push(("agent-type".into(), "PromptCenter".into()));
        base_headers.push(("x-language".into(), "zh-cn".into()));
    } else {
        base_headers.push(("app-id".into(), "CodeAgent3.0".into()));
        base_headers.push(("agent-type".into(), "PromptCenter".into()));
        base_headers.push(("plugin-name".into(), "codemate_vscode".into()));
        base_headers.push(("plugin-version".into(), "26.9.902".into()));
        base_headers.push(("ide-name".into(), "VSCode".into()));
        base_headers.push(("ide-version".into(), "26.9.902".into()));
        base_headers.push(("x-ot-client-type".into(), "IDE".into()));
    }
    let host_of_url = url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string();
    let (auth, sign_headers) = sign_codearts(ak, sk, method.as_str(), &host_of_url, path, query, &base_headers, &body_str, st);
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let full = if query.is_empty() { url.clone() } else { format!("{url}?{query}") };
    let mut req = client.request(method, &full);
    for (k, v) in &sign_headers {
        req = req.header(k, v);
    }
    req = req.header("Authorization", auth);
    if !body_str.is_empty() {
        req = req.body(body_str);
    }
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() { 401 => return Err(PlatformError::Auth), 429 => return Err(PlatformError::Rate), 403 => return Err(PlatformError::Forbidden), _ => {} }
    resp.json::<Value>().await.map_err(|e| PlatformError::Format(e.to_string()))
}

fn iso_ms(s: &str) -> Option<i64> {
    // ISO8601 → epoch ms（无完整解析器，覆盖平台返回的 Z / +08:00 两种形态）
    let s = s.trim();
    let (body, off_ms) = if let Some(p) = s.rfind(['+', '-']) {
        let off = &s[p..];
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let hh: i64 = off.get(1..3)?.parse().ok()?;
        let mm: i64 = off.get(4..6)?.parse().ok()?;
        (&s[..p], sign * (hh * 3600 + mm * 60) * 1000)
    } else {
        (s, 0)
    };
    let digits: String = body.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.len() < 14 { return None; }
    let y: i64 = digits.get(0..4)?.parse().ok()?;
    let mo: i64 = digits.get(4..6)?.parse().ok()?;
    let d: i64 = digits.get(6..8)?.parse().ok()?;
    let hh: i64 = digits.get(8..10)?.parse().ok()?;
    let mi: i64 = digits.get(10..12)?.parse().ok()?;
    let ss: i64 = digits.get(12..14)?.parse().ok()?;
    let days = crate::adapters::days_from_civil(y, mo, d);
    Some((days * 86400 + hh * 3600 + mi * 60 + ss) * 1000 - off_ms)
}

fn idempotent_key(uid: &str, day: &str, campaign_id: &str) -> String {
    hex_lower(&Sha256::digest(format!("{uid}:{day}:{campaign_id}").as_bytes()))
}

#[async_trait::async_trait]
impl Adapter for CodeArtsAdapter {
    fn provider(&self) -> &'static str { if self.desktop { "codearts-desktop" } else { "codearts" } }

    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        // GET /v5/caller-identity（on STS）
        let v = ops_request(c, self.desktop, Method::GET, "/v5/caller-identity", "", Value::Null, true).await?;
        Ok(v)
    }

    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let q = format!("channel={}", channel(self.desktop));
        let v = ops_request(c, self.desktop, Method::GET, "/v1/ops/delivery", &q, Value::Null, false).await?;
        let items = v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
        // vendor daily(): USER_LOGIN + campaignId + (triggerMode 空或 MANUAL_CLAIM) + 时间窗内
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 * 1000;
        let daily: Vec<Value> = items.into_iter().filter(|x| {
            let typ = x.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let cid = x.get("campaignId").and_then(|t| t.as_str()).unwrap_or("");
            if typ != "USER_LOGIN" || cid.is_empty() { return false; }
            let tm = x.pointer("/extra/triggerMode").and_then(|t| t.as_str()).unwrap_or("");
            if !tm.is_empty() && tm != "MANUAL_CLAIM" { return false; }
            let start = x.pointer("/extra/startTime").and_then(|s| s.as_str()).and_then(|s| iso_ms(s));
            let end = x.pointer("/extra/endTime").and_then(|s| s.as_str()).and_then(|s| iso_ms(s));
            !(start.map(|s| s > now_ms).unwrap_or(false)) && !(end.map(|e| e <= now_ms).unwrap_or(false))
        }).collect();
        let granted_status = |s: &str| if self.desktop {
            ["CLAIMED", "CONFIRMED", "CONSUMED"].contains(&s)
        } else {
            ["CONFIRMED", "CONSUMED"].contains(&s)
        };
        let checked = !daily.is_empty() && daily.iter().all(|x| granted_status(x.get("status").and_then(|t| t.as_str()).unwrap_or("")));
        let claimable = daily.iter().any(|x| x.get("status").and_then(|t| t.as_str()) == Some("ELIGIBLE") && x.get("claimable").and_then(|b| b.as_bool()) == Some(true));
        Ok(CheckinState { checked, claimable, raw: json!({ "items": daily }), ..Default::default() })
    }

    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: None });
        }
        let items = st.raw.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: None });
        }
        let day = crate::adapters::day_cn();
        let mut amount: Option<f64> = None;
        for it in &items {
            let cid = it.get("campaignId").and_then(|t| t.as_str()).unwrap_or_default();
            let body = json!({
                "campaignId": cid,
                "channel": channel(self.desktop),
                "idempotentKey": idempotent_key(&c.uid, &day, cid),
            });
            let d = ops_request(c, self.desktop, Method::POST, "/v1/ops/claim", "", body, false).await?;
            amount = Some(amount.unwrap_or(0.0)
                + d.get("amount").and_then(|a| a.as_f64()).unwrap_or(0.0)
                + d.get("benefitAmount").and_then(|a| a.as_f64()).unwrap_or(0.0));
            // confirm
            let confirm_body = if self.desktop {
                let bid = d.get("id").or_else(|| d.get("userBenefitId")).cloned().unwrap_or(Value::Null);
                json!({ "userBenefitId": bid })
            } else {
                json!({ "campaignId": cid })
            };
            let _ = ops_request(c, self.desktop, Method::POST, "/v1/ops/confirm", "", confirm_body, false).await?;
        }
        Ok(Receipt { status: "success".into(), amount, message: None })
    }

    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let v = ops_request(c, self.desktop, Method::GET, "/snap-manager/v1/statistics/plugin", "", Value::Null, false).await?;
        let metrics = v.get("metrics").and_then(|m| m.as_array()).cloned().unwrap_or_default();
        let metric = metrics.iter().find(|m| m.get("name").and_then(|n| n.as_str()) == Some("usageTotalPackageCredit"));
        let Some(metric) = metric else {
            return Ok(Quota { remaining: None, parts: json!([]), sampled_at: now_iso() });
        };
        let num = |x: &Value| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()));
        let total = metric.get("package_credit_amount").and_then(num);
        let used = metric.get("package_credit_used").and_then(num);
        let remain = metric.get("package_credit_remain").and_then(num)
            .or_else(|| match (total, used) { (Some(t), Some(u)) => Some((t - u).max(0.0)), _ => None });
        Ok(Quota { remaining: remain, parts: json!([]), sampled_at: now_iso() })
    }

    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let Some(rt) = c.meta.get("refresh_token").and_then(|r| r.as_str()) else { return Err(PlatformError::Auth) };
        let jwk = c.meta.get("privateJwk").cloned().unwrap_or(Value::Null);
        let v = sts_token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            ("client_id", if self.desktop { "codearts" } else { "codearts-agent" }),
        ], &jwk).await?;
        let mut out = c.clone();
        if let Some(cred) = v.get("credentials") {
            out.meta["access_key_id"] = cred.get("access_key_id").cloned().unwrap_or(Value::Null);
            out.meta["secret_access_key"] = cred.get("secret_access_key").cloned().unwrap_or(Value::Null);
            out.meta["security_token"] = cred.get("security_token").cloned().unwrap_or(Value::Null);
        }
        Ok(out)
    }
}
